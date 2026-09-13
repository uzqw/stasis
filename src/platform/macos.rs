use crossbeam_channel::Sender;

use crate::platform::{BackendEvent, Grab};

pub fn grab(_tx: Sender<BackendEvent>) -> anyhow::Result<Grab> {
    anyhow::bail!("macOS backend not yet implemented")
}
