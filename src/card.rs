use drm::{control::Device as ControlDevice, Device};
use std::{
    fs,
    os::fd::{AsFd, BorrowedFd},
};

pub struct Card(fs::File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl Card {
    pub fn open(path: impl Into<String>) -> Self {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        options.write(true);
        Self(options.open(path.into()).expect("failed to open drm device"))
    }
}

impl Device for Card {
}
impl ControlDevice for Card {
}
