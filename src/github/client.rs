use anyhow::{Context, Result};
use octocrab::Octocrab;

use crate::config::Config;

pub struct Client {
    pub octocrab: Octocrab,
    pub owner: String,
    pub repo: String,
}

impl Client {
    pub fn new(config: &Config) -> Result<Self> {
        let token = config.github_token()?;
        let octocrab = Octocrab::builder()
            .personal_token(token)
            .build()
            .context("Failed to create GitHub client")?;

        Ok(Self {
            octocrab,
            owner: config.repo_owner.clone(),
            repo: config.repo_name.clone(),
        })
    }
}
