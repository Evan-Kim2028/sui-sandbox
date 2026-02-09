use super::*;

impl ReplayCmd {
    #[cfg(not(feature = "igloo"))]
    pub(super) async fn build_replay_state_hybrid(
        &self,
        _provider: &HistoricalStateProvider,
        _verbose: bool,
    ) -> Result<ReplayState> {
        Err(anyhow!(
            "igloo hybrid loader is not enabled in this build (rebuild with `--features igloo`)"
        ))
    }
}
