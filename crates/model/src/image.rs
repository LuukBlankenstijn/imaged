use derive_more::{Constructor, Display, FromStr, IsVariant};
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Display, FromStr, IsVariant, PartialEq, Eq, Serialize, Deserialize,
)]
#[display(rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ImageStatus {
    Empty,
    Capturing,
    Importing,
    Ready,
    Faulted,
}

#[derive(Clone, Debug, Constructor, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePartition {
    pub id: i64,
    pub partition_number: i64,
    pub fstype: String,
    pub size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ImageStatus; 5] = [
        ImageStatus::Empty,
        ImageStatus::Capturing,
        ImageStatus::Importing,
        ImageStatus::Ready,
        ImageStatus::Faulted,
    ];

    #[test]
    fn image_status_round_trips_through_display_and_from_str_for_every_variant() {
        for s in ALL {
            assert_eq!(s.to_string().parse::<ImageStatus>().unwrap(), s);
        }
    }

    #[test]
    fn image_status_display_is_lowercase() {
        assert_eq!(ImageStatus::Empty.to_string(), "empty");
        assert_eq!(ImageStatus::Capturing.to_string(), "capturing");
        assert_eq!(ImageStatus::Importing.to_string(), "importing");
        assert_eq!(ImageStatus::Ready.to_string(), "ready");
        assert_eq!(ImageStatus::Faulted.to_string(), "faulted");
    }

    #[test]
    fn image_status_serializes_as_its_display_name() {
        for s in ALL {
            assert_eq!(serde_json::to_string(&s).unwrap(), format!("\"{s}\""));
        }
    }
}
