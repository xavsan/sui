// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::config::Config;
use crate::latest_eth_syncer::LatestEthSyncer;
use crate::metrics::BridgeIndexerMetrics;
use crate::postgres_manager::get_connection_pool;
use crate::postgres_manager::get_latest_eth_token_transfer;
use crate::postgres_manager::{write, PgPool};
use crate::sui_worker::SuiBridgeWorker;
use crate::{BridgeDataSource, TokenTransfer, TokenTransferData, TokenTransferStatus};
use anyhow::Result;
use ethers::providers::Provider;
use ethers::providers::{Http, Middleware};
use ethers::types::Address as EthAddress;
use mysten_metrics::spawn_logged_monitored_task;
use std::collections::HashMap;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use sui_bridge::abi::{EthBridgeEvent, EthSuiBridgeEvents};
use sui_bridge::config;
use sui_bridge::types::EthLog;
use sui_bridge::{eth_client::EthClient, eth_syncer::EthSyncer};
use tracing::info;
use tracing::log::error;

pub struct EthBridgeWorker {
    provider: Arc<Provider<Http>>,
    pg_pool: PgPool,
    metrics: BridgeIndexerMetrics,
    bridge_address: EthAddress,
    config: Config,
}

impl EthBridgeWorker {
    pub fn new(pg_pool: PgPool, metrics: BridgeIndexerMetrics, config: Config) -> Self {
        let bridge_address = match EthAddress::from_str(&config.eth_sui_bridge_contract_address) {
            Ok(addr) => addr,
            Err(e) => {
                panic!("Invalid Ethereum address: {}", e);
            }
        };

        let provider = Arc::new(
            ethers::prelude::Provider::<ethers::providers::Http>::try_from(&config.eth_rpc_url)
                .unwrap_or_else(|_| {
                    panic!(
                        "Cannot create Ethereum HTTP provider, URL: {}",
                        &config.eth_rpc_url
                    )
                })
                .interval(std::time::Duration::from_millis(2000)),
        );
        Self {
            provider,
            pg_pool,
            metrics,
            bridge_address,
            config,
        }
    }

    pub async fn run(self, config: Config) -> Result<()> {
        let eth_client = Arc::new(
            EthClient::<ethers::providers::Http>::new(
                &config.eth_rpc_url,
                HashSet::from_iter(vec![self.bridge_address]),
            )
            .await?,
        );

        self.start_processing_finalized_eth_events(eth_client.clone())
            .await?;
        self.start_processing_unfinalized_eth_events(eth_client.clone())
            .await?;

        Ok(())
    }

    async fn start_processing_finalized_eth_events(
        &self,
        eth_client: Arc<EthClient<Http>>,
    ) -> Result<()> {
        let newest_finalized_block = match get_latest_eth_token_transfer(&self.pg_pool, true)? {
            Some(transfer) => transfer.block_height as u64,
            None => self.config.start_block,
        };

        info!("Starting from finalized block: {}", newest_finalized_block);

        let finalized_contract_addresses =
            HashMap::from_iter(vec![(self.bridge_address, newest_finalized_block)]);

        let (_task_handles, eth_events_rx, _) =
            EthSyncer::new(eth_client, finalized_contract_addresses)
                .run()
                .await
                .expect("Failed to start eth syncer");

        let _finalized_task_handle = spawn_logged_monitored_task!(
            self._process_eth_events(eth_events_rx, true),
            "finalized indexer handler"
        );

        Ok(())
    }

    async fn start_processing_unfinalized_eth_events(
        &self,
        eth_client: Arc<EthClient<Http>>,
    ) -> Result<()> {
        let newest_unfinalized_block_recorded =
            match get_latest_eth_token_transfer(&self.pg_pool, false)? {
                Some(transfer) => transfer.block_height as u64,
                None => self.config.start_block,
            };

        info!(
            "Starting from unfinalized block: {}",
            newest_unfinalized_block_recorded
        );

        let unfinalized_contract_addresses = HashMap::from_iter(vec![(
            self.bridge_address,
            newest_unfinalized_block_recorded,
        )]);

        let (_task_handles, eth_events_rx) = LatestEthSyncer::new(
            eth_client,
            self.provider.clone(),
            unfinalized_contract_addresses.clone(),
        )
        .run()
        .await
        .expect("Failed to start eth syncer");

        let _unfinalized_task_handle = spawn_logged_monitored_task!(
            self._process_eth_events(eth_events_rx, false),
            "unfinalized indexer handler"
        );

        Ok(())
    }

    async fn _process_eth_events(
        &self,
        mut eth_events_rx: mysten_metrics::metered_channel::Receiver<(
            EthAddress,
            u64,
            Vec<EthLog>,
        )>,
        finalized: bool,
    ) {
        while let Some((_, _, logs)) = eth_events_rx.recv().await {
            for log in logs.iter() {
                let eth_bridge_event = EthBridgeEvent::try_from_eth_log(log);
                if eth_bridge_event.is_none() {
                    continue;
                }
                let bridge_event = eth_bridge_event.unwrap();
                let block_number = log.block_number;
                let block = self
                    .provider
                    .get_block(log.block_number)
                    .await
                    .unwrap()
                    .unwrap();
                let timestamp = block.timestamp.as_u64() * 1000;
                let transaction = self
                    .provider
                    .get_transaction(log.tx_hash)
                    .await
                    .unwrap()
                    .unwrap();
                let gas = transaction.gas;
                let tx_hash = log.tx_hash;

                let transfer: TokenTransfer = match bridge_event {
                    EthBridgeEvent::EthSuiBridgeEvents(bridge_event) => match bridge_event {
                        EthSuiBridgeEvents::TokensDepositedFilter(bridge_event) => {
                            info!(
                                "Observed {} Eth Deposit",
                                if finalized {
                                    "Finalized"
                                } else {
                                    "Unfinalized"
                                }
                            );
                            TokenTransfer {
                                chain_id: bridge_event.source_chain_id,
                                nonce: bridge_event.nonce,
                                block_height: block_number,
                                timestamp_ms: timestamp,
                                txn_hash: tx_hash.as_bytes().to_vec(),
                                txn_sender: bridge_event.sender_address.as_bytes().to_vec(),
                                status: if finalized {
                                    TokenTransferStatus::Deposited
                                } else {
                                    TokenTransferStatus::DepositedUnfinalized
                                },
                                gas_usage: gas.as_u64() as i64,
                                data_source: BridgeDataSource::Eth,
                                data: Some(TokenTransferData {
                                    sender_address: bridge_event.sender_address.as_bytes().to_vec(),
                                    destination_chain: bridge_event.destination_chain_id,
                                    recipient_address: bridge_event.recipient_address.to_vec(),
                                    token_id: bridge_event.token_id,
                                    amount: bridge_event.sui_adjusted_amount,
                                }),
                            }
                        }
                        EthSuiBridgeEvents::TokensClaimedFilter(bridge_event) => {
                            // Only write unfinalized claims
                            if finalized {
                                continue;
                            }
                            info!("Observed Unfinalized Eth Claim");
                            TokenTransfer {
                                chain_id: bridge_event.source_chain_id,
                                nonce: bridge_event.nonce,
                                block_height: block_number,
                                timestamp_ms: timestamp,
                                txn_hash: tx_hash.as_bytes().to_vec(),
                                txn_sender: bridge_event.sender_address.to_vec(),
                                status: TokenTransferStatus::Claimed,
                                gas_usage: gas.as_u64() as i64,
                                data_source: BridgeDataSource::Eth,
                                data: None,
                            }
                        }
                        EthSuiBridgeEvents::PausedFilter(_)
                        | EthSuiBridgeEvents::UnpausedFilter(_)
                        | EthSuiBridgeEvents::UpgradedFilter(_)
                        | EthSuiBridgeEvents::InitializedFilter(_) => {
                            continue;
                        }
                    },
                    EthBridgeEvent::EthBridgeCommitteeEvents(_)
                    | EthBridgeEvent::EthBridgeLimiterEvents(_)
                    | EthBridgeEvent::EthBridgeConfigEvents(_)
                    | EthBridgeEvent::EthCommitteeUpgradeableContractEvents(_) => {
                        continue;
                    }
                };

                if let Err(e) = write(&self.pg_pool, vec![transfer]) {
                    error!("Error writing token transfer to database: {:?}", e);
                }
            }
        }

        panic!("Eth event stream ended unexpectedly");
    }
}
