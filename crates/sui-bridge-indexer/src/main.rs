// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use clap::*;
use ethers::types::Address as EthAddress;
use mysten_metrics::spawn_logged_monitored_task;
use mysten_metrics::start_prometheus_server;
use prometheus::Registry;
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use sui_bridge::{eth_client::EthClient, eth_syncer::EthSyncer};
use sui_bridge_indexer::latest_eth_syncer::LatestEthSyncer;
use sui_bridge_indexer::postgres_manager::get_connection_pool;
use sui_bridge_indexer::postgres_manager::get_latest_eth_token_transfer;
use sui_bridge_indexer::sui_worker::SuiBridgeWorker;
use sui_bridge_indexer::{config::load_config, metrics::BridgeIndexerMetrics};
use sui_data_ingestion_core::{
    DataIngestionMetrics, FileProgressStore, IndexerExecutor, ReaderOptions, WorkerPool,
};
use tokio::sync::oneshot;
use tracing::info;

#[derive(Parser, Clone, Debug)]
struct Args {
    /// Path to a yaml config
    #[clap(long, short)]
    config_path: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = telemetry_subscribers::TelemetryConfig::new()
        .with_env()
        .init();

    let args = Args::parse();

    // load config
    let config_path = if let Some(path) = args.config_path {
        path.clone_from(args.config_path)
    } else {
        env::current_dir()
            .expect("Current directory is invalid.")
            .join("config.yaml")
    };
    let config = load_config(&config_path).unwrap();

    // Init metrics server
    let registry_service = start_prometheus_server(
        format!("{}:{}", config.metric_url, config.metric_port,)
            .parse()
            .unwrap_or_else(|err| panic!("Failed to parse metric address: {}", err)),
    );
    let registry: Registry = registry_service.default_registry();

    mysten_metrics::init_metrics(&registry);

    info!(
        "Metrics server started at {}::{}",
        config.metric_url, config.metric_port
    );
    let indexer_meterics = BridgeIndexerMetrics::new(&registry);

    start_processing_sui_checkpoints(&config, indexer_meterics).await?;

    let worker = sui_bridge_indexer::eth_worker::EthBridgeWorker::new(
        get_connection_pool(config.db_url.clone()),
        indexer_meterics,
        config,
    );
    worker.run(config).await?;

    Ok(())
}

async fn start_processing_sui_checkpoints(
    config: &sui_bridge_indexer::config::Config,
    indexer_meterics: BridgeIndexerMetrics,
) -> Result<()> {
    // metrics init
    let (_exit_sender, exit_receiver) = oneshot::channel();
    let metrics = DataIngestionMetrics::new(&Registry::new());

    let progress_store = FileProgressStore::new(config.progress_store_file.clone().into());
    let mut executor = IndexerExecutor::new(progress_store, 1 /* workflow types */, metrics);

    let indexer_metrics_cloned = indexer_meterics.clone();

    let worker_pool = WorkerPool::new(
        SuiBridgeWorker::new(vec![], config.db_url.clone(), indexer_metrics_cloned),
        "bridge worker".into(),
        config.concurrency as usize,
    );
    executor.register(worker_pool).await?;
    executor
        .run(
            config.checkpoints_path.clone().into(),
            Some(config.remote_store_url.clone()),
            vec![], // optional remote store access options
            ReaderOptions::default(),
            exit_receiver,
        )
        .await?;

    Ok(())
}
