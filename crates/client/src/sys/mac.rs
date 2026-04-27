use crate::error::Error;

pub fn get_mac() -> Result<String, Error> {
    Ok(mac_address::get_mac_address()?
        .ok_or(Error::NoMac)?
        .to_string())
}
