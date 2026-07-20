use std::io;

use super::KeychainBlob;

pub const KEYCHAIN_SERVICE: &str = "run.lns";
pub const KEYCHAIN_ITEM: &str = "credentials";

pub struct KeyringBlob {
    entry: keyring::Entry,
}

impl KeyringBlob {
    pub fn open() -> io::Result<Self> {
        keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ITEM)
            .map(|entry| Self { entry })
            .map_err(io::Error::other)
    }
}

impl KeychainBlob for KeyringBlob {
    fn read(&self) -> io::Result<Option<String>> {
        match self.entry.get_password() {
            Ok(blob) => Ok(Some(blob)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(io::Error::other(e)),
        }
    }

    fn write(&self, blob: &str) -> io::Result<()> {
        self.entry.set_password(blob).map_err(io::Error::other)
    }
}
