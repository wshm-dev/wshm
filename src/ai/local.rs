// Local AI inference (llama.cpp) is not compiled in this build.
// To enable local inference, see the `local-ai` feature flag.

use anyhow::{bail, Result};
use serde::de::DeserializeOwned;

pub struct LocalClient;

impl LocalClient {
    pub fn new(_model: &str) -> Result<Self> {
        bail!(
            "Local AI inference is not available in this build.\n\
             Configure a remote provider in [ai] section:\n\
             provider = \"anthropic\"  # or \"openai\", \"google\"\n\
             model    = \"claude-sonnet-4-5\""
        )
    }

    pub fn complete<T: DeserializeOwned>(&self, _system: &str, _user: &str) -> Result<T> {
        bail!("Local AI inference not available")
    }
}
