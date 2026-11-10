use std::{collections::HashMap, fmt, net::IpAddr, str::FromStr};

use imaged_model::agent::MacAddress;
use serde::Deserialize;

use crate::pxe::Arch;

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BootClient {
    pub arch: Arch,
    pub ip: IpAddr,
    pub mac: Option<MacAddress>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

impl BootClient {
    pub fn new(arch: Arch, ip: IpAddr) -> Self {
        Self {
            arch,
            ip,
            mac: None,
            manufacturer: None,
            product: None,
        }
    }
}

pub trait ProfileSelector: fmt::Debug + Send + Sync {
    fn profile(&self, client: &BootClient) -> Option<ProfileName>;
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "String")]
pub struct ProfileName(String);

#[derive(Debug)]
pub struct InvalidProfileName(String);

impl fmt::Display for InvalidProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "boot profile name {:?} must be letters, digits, '-' and '_'",
            self.0
        )
    }
}

impl std::error::Error for InvalidProfileName {}

impl TryFrom<String> for ProfileName {
    type Error = InvalidProfileName;

    fn try_from(name: String) -> Result<Self, InvalidProfileName> {
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if valid {
            Ok(Self(name))
        } else {
            Err(InvalidProfileName(name))
        }
    }
}

impl FromStr for ProfileName {
    type Err = InvalidProfileName;

    fn from_str(name: &str) -> Result<Self, InvalidProfileName> {
        name.to_string().try_into()
    }
}

impl ProfileName {
    pub fn of(arch: Arch) -> Self {
        Self(arch.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Default)]
pub(crate) struct ProductSelector(pub(crate) HashMap<String, ProfileName>);

impl ProfileSelector for ProductSelector {
    fn profile(&self, client: &BootClient) -> Option<ProfileName> {
        self.0.get(client.product.as_deref()?.trim()).cloned()
    }
}
