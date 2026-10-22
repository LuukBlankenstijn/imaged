use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};

use crate::domain::task::{SCRIPT_OUTPUT_TAIL_BYTES, TaskRepository};
use imaged_model::error::Result;

const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

pub struct ScriptRunKey {
    pub task_id: i64,
    pub host_id: i64,
    pub position: i64,
}

pub async fn record_output<S>(
    task_repo: &dyn TaskRepository,
    run: ScriptRunKey,
    output: S,
) -> Result
where
    S: Stream<Item = Result<Bytes>>,
{
    let mut output = std::pin::pin!(output);
    let mut pending = Vec::new();
    let mut flush = tokio::time::interval(FLUSH_INTERVAL);
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            chunk = output.next() => match chunk {
                Some(chunk) => {
                    pending.extend_from_slice(&chunk?);
                    if pending.len() >= SCRIPT_OUTPUT_TAIL_BYTES {
                        append(task_repo, &run, &decoded_prefix(&mut pending)).await?;
                    }
                }
                None => break,
            },
            _ = flush.tick() => append(task_repo, &run, &decoded_prefix(&mut pending)).await?,
        }
    }
    let mut rest = decoded_prefix(&mut pending);
    rest.push_str(&String::from_utf8_lossy(&pending));
    append(task_repo, &run, &rest).await
}

async fn append(task_repo: &dyn TaskRepository, run: &ScriptRunKey, text: &str) -> Result {
    if text.is_empty() {
        return Ok(());
    }
    task_repo
        .append_script_output(run.task_id, run.host_id, run.position, text)
        .await
}

fn decoded_prefix(pending: &mut Vec<u8>) -> String {
    let mut text = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(valid) => {
                text.push_str(valid);
                pending.clear();
                return text;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(&String::from_utf8_lossy(&pending[..valid]));
                match error.error_len() {
                    Some(invalid) => {
                        text.push(char::REPLACEMENT_CHARACTER);
                        pending.drain(..valid + invalid);
                    }
                    None => {
                        pending.drain(..valid);
                        return text;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_split_across_chunks_waits_for_its_remaining_bytes() {
        let euro = "€".as_bytes();
        let mut pending = [b"cost: ".as_slice(), &euro[..2]].concat();

        assert_eq!(decoded_prefix(&mut pending), "cost: ");
        assert_eq!(pending, &euro[..2]);

        pending.extend_from_slice(&euro[2..]);
        assert_eq!(decoded_prefix(&mut pending), "€");
        assert!(pending.is_empty());
    }

    #[test]
    fn invalid_bytes_become_replacement_characters_without_losing_what_follows() {
        let mut pending = b"a\xffb\xc3".to_vec();

        assert_eq!(decoded_prefix(&mut pending), "a\u{fffd}b");
        assert_eq!(pending, b"\xc3");
    }
}
