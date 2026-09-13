use crossbeam_channel::Sender;

use crate::platform::{BackendEvent, Grab};

pub fn grab(_tx: Sender<BackendEvent>) -> anyhow::Result<Grab> {
    anyhow::bail!("Windows backend not yet implemented")
}
