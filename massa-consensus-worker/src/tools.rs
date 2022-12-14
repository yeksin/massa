use crate::consensus_worker::ConsensusWorker;
use massa_consensus_exports::settings::ConsensusConfig;
use massa_consensus_exports::{
    commands::{ConsensusCommand, ConsensusManagementCommand},
    error::{ConsensusError, ConsensusResult as Result},
    events::ConsensusEvent,
    settings::{ConsensusChannels, ConsensusWorkerChannels},
    ConsensusCommandSender, ConsensusEventReceiver, ConsensusManager,
};
use massa_graph::{settings::GraphConfig, BlockGraph, BootstrapableGraph};
use massa_storage::Storage;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Creates a new consensus controller.
///
/// # Arguments
/// * `cfg`: consensus configuration
/// * `protocol_command_sender`: a `ProtocolCommandSender` instance to send commands to Protocol.
/// * `protocol_event_receiver`: a `ProtocolEventReceiver` instance to receive events from Protocol.
#[allow(clippy::too_many_arguments)]
pub async fn start_consensus_controller(
    cfg: ConsensusConfig,
    channels: ConsensusChannels,
    boot_graph: Option<BootstrapableGraph>,
    storage: Storage,
    clock_compensation: i64,
) -> Result<(
    ConsensusCommandSender,
    ConsensusEventReceiver,
    ConsensusManager,
)> {
    debug!("starting consensus controller");
    massa_trace!(
        "consensus.consensus_controller.start_consensus_controller",
        {}
    );

    // todo that is checked when loading the config, should be removed
    // ensure that the parameters are sane
    if cfg.thread_count == 0 {
        return Err(ConsensusError::ConfigError(
            "thread_count should be strictly more than 0".to_string(),
        ));
    }
    if cfg.t0 == 0.into() {
        return Err(ConsensusError::ConfigError(
            "t0 should be strictly more than 0".to_string(),
        ));
    }
    if cfg.t0.checked_rem_u64(cfg.thread_count as u64)? != 0.into() {
        return Err(ConsensusError::ConfigError(
            "thread_count should divide t0".to_string(),
        ));
    }

    // start worker
    let block_db = BlockGraph::new(
        GraphConfig::from(&cfg),
        boot_graph,
        storage.clone_without_refs(),
        channels.selector_controller.clone(),
    )
    .await?;
    let (command_tx, command_rx) = mpsc::channel::<ConsensusCommand>(cfg.channel_size);
    let (event_tx, event_rx) = mpsc::channel::<ConsensusEvent>(cfg.channel_size);
    let (manager_tx, manager_rx) = mpsc::channel::<ConsensusManagementCommand>(1);
    let cfg_copy = cfg.clone();
    let join_handle = tokio::spawn(async move {
        let res = ConsensusWorker::new(
            cfg_copy,
            ConsensusWorkerChannels {
                protocol_command_sender: channels.protocol_command_sender,
                protocol_event_receiver: channels.protocol_event_receiver,
                execution_controller: channels.execution_controller,
                pool_command_sender: channels.pool_command_sender,
                selector_controller: channels.selector_controller,
                controller_command_rx: command_rx,
                controller_event_tx: event_tx,
                controller_manager_rx: manager_rx,
            },
            block_db,
            clock_compensation,
        )
        .await?
        .run_loop()
        .await;
        match res {
            Err(err) => {
                error!("consensus worker crashed: {}", err);
                Err(err)
            }
            Ok(v) => {
                info!("consensus worker finished cleanly");
                Ok(v)
            }
        }
    });
    Ok((
        ConsensusCommandSender(command_tx),
        ConsensusEventReceiver(event_rx),
        ConsensusManager {
            manager_tx,
            join_handle,
        },
    ))
}
