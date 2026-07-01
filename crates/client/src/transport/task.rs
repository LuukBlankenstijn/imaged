use serde_json::json;

use crate::transport::ApiClient;

impl ApiClient {
    pub async fn mark_task_finished(&self, task_id: i64) -> anyhow::Result<()> {
        let url = self.url(&format!("client/tasks/{}/finished", task_id))?;

        self.send(self.client.post(url), "mark_deploy_finished")
            .await?;
        Ok(())
    }

    pub async fn mark_task_failed(&self, task_id: i64, err: &str) -> anyhow::Result<()> {
        let url = self.url(&format!("client/tasks/{}/faulted", task_id))?;

        self.send(
            self.client.post(url).json(&json!({
                "error": err
            })),
            "mark_deploy_failed",
        )
        .await?;
        Ok(())
    }
}
