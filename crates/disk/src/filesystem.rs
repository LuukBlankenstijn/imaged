use std::fmt;
use std::str::FromStr;

use anyhow::bail;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filesystem {
    Ext2,
    Ext3,
    Ext4,
    Vfat,
}

impl Filesystem {
    pub fn detect(fstype: Option<&str>) -> anyhow::Result<Option<Self>> {
        fstype.map(str::parse).transpose()
    }

    pub fn is_ext(self) -> bool {
        matches!(self, Self::Ext2 | Self::Ext3 | Self::Ext4)
    }

    pub fn partclone_binary(self) -> &'static str {
        if self.is_ext() {
            "partclone.extfs"
        } else {
            "partclone.vfat"
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ext2 => "ext2",
            Self::Ext3 => "ext3",
            Self::Ext4 => "ext4",
            Self::Vfat => "vfat",
        }
    }
}

impl FromStr for Filesystem {
    type Err = anyhow::Error;

    fn from_str(fstype: &str) -> anyhow::Result<Self> {
        Ok(match fstype {
            "ext2" => Self::Ext2,
            "ext3" => Self::Ext3,
            "ext4" => Self::Ext4,
            "vfat" | "fat32" | "fat16" => Self::Vfat,
            other => bail!("filesystem not supported: {other}"),
        })
    }
}

impl fmt::Display for Filesystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_filesystems_are_cloned_with_partclone_extfs() {
        for (fstype, filesystem) in [
            ("ext2", Filesystem::Ext2),
            ("ext3", Filesystem::Ext3),
            ("ext4", Filesystem::Ext4),
        ] {
            let parsed: Filesystem = fstype.parse().unwrap();
            assert_eq!(parsed, filesystem);
            assert!(parsed.is_ext());
            assert_eq!(parsed.partclone_binary(), "partclone.extfs");
            assert_eq!(parsed.to_string(), fstype);
        }
    }

    #[test]
    fn fat_variants_are_all_cloned_as_vfat() {
        for fstype in ["vfat", "fat32", "fat16"] {
            let parsed: Filesystem = fstype.parse().unwrap();
            assert_eq!(parsed, Filesystem::Vfat);
            assert!(!parsed.is_ext());
            assert_eq!(parsed.partclone_binary(), "partclone.vfat");
            assert_eq!(parsed.to_string(), "vfat");
        }
    }

    #[test]
    fn unmapped_filesystems_are_rejected() {
        for fstype in ["ntfs", "xfs", "btrfs", "swap", "crypto_LUKS", ""] {
            let e = fstype.parse::<Filesystem>().unwrap_err();
            assert_eq!(e.to_string(), format!("filesystem not supported: {fstype}"));
        }
    }

    #[test]
    fn filesystem_names_are_case_sensitive() {
        for fstype in ["EXT4", "Ext4", "VFAT"] {
            let e = fstype.parse::<Filesystem>().unwrap_err();
            assert_eq!(e.to_string(), format!("filesystem not supported: {fstype}"));
        }
    }

    #[test]
    fn detect_skips_unformatted_partitions_and_rejects_unsupported_ones() {
        assert_eq!(Filesystem::detect(None).unwrap(), None);
        assert_eq!(
            Filesystem::detect(Some("ext4")).unwrap(),
            Some(Filesystem::Ext4)
        );
        assert_eq!(
            Filesystem::detect(Some("swap")).unwrap_err().to_string(),
            "filesystem not supported: swap"
        );
    }
}
