use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::sol_types;
use futures_util::Stream;
use sqlx::SqlitePool;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tracing::info;

use st0x_broker::Broker;

use crate::bindings::IOrderBookV4::{ClearV2, TakeOrderV2};
use crate::env::Env;
use crate::onchain::trade::TradeEvent;
use crate::symbol::cache::SymbolCache;

use super::{
    Conductor, spawn_event_processor, spawn_onchain_event_receiver, spawn_order_poller,
    spawn_periodic_accumulated_position_check, spawn_queue_processor,
};

type ClearStream = Box<dyn Stream<Item = Result<(ClearV2, Log), sol_types::Error>> + Unpin + Send>;
type TakeStream =
    Box<dyn Stream<Item = Result<(TakeOrderV2, Log), sol_types::Error>> + Unpin + Send>;

struct CommonFields<P, B> {
    env: Env,
    pool: SqlitePool,
    cache: SymbolCache,
    provider: P,
    broker: B,
}

pub(crate) struct Initial;

pub(crate) struct WithDexStreams {
    clear_stream: ClearStream,
    take_stream: TakeStream,
    event_sender: UnboundedSender<(TradeEvent, Log)>,
    event_receiver: UnboundedReceiver<(TradeEvent, Log)>,
}

pub(crate) struct ConductorBuilder<P, B, State> {
    common: CommonFields<P, B>,
    state: State,
}

impl<P: Provider + Clone + Send + 'static, B: Broker + Clone + Send + 'static>
    ConductorBuilder<P, B, Initial>
{
    pub(crate) fn new(
        env: Env,
        pool: SqlitePool,
        cache: SymbolCache,
        provider: P,
        broker: B,
    ) -> Self {
        Self {
            common: CommonFields {
                env,
                pool,
                cache,
                provider,
                broker,
            },
            state: Initial,
        }
    }

    pub(crate) fn with_dex_event_streams(
        self,
        clear_stream: impl Stream<Item = Result<(ClearV2, Log), sol_types::Error>>
        + Unpin
        + Send
        + 'static,
        take_stream: impl Stream<Item = Result<(TakeOrderV2, Log), sol_types::Error>>
        + Unpin
        + Send
        + 'static,
    ) -> ConductorBuilder<P, B, WithDexStreams> {
        let (event_sender, event_receiver) =
            tokio::sync::mpsc::unbounded_channel::<(TradeEvent, Log)>();

        ConductorBuilder {
            common: self.common,
            state: WithDexStreams {
                clear_stream: Box::new(clear_stream),
                take_stream: Box::new(take_stream),
                event_sender,
                event_receiver,
            },
        }
    }
}

impl<P: Provider + Clone + Send + 'static, B: Broker + Clone + Send + 'static>
    ConductorBuilder<P, B, WithDexStreams>
{
    pub(crate) async fn spawn(self) -> Conductor {
        info!("Starting conductor orchestration");

        let broker_maintenance = self.common.broker.run_broker_maintenance().await;

        if broker_maintenance.is_some() {
            info!("Started broker maintenance tasks");
        } else {
            info!("No broker maintenance tasks needed");
        }

        let order_poller = spawn_order_poller(
            &self.common.env,
            &self.common.pool,
            self.common.broker.clone(),
        );
        let dex_event_receiver = spawn_onchain_event_receiver(
            self.state.event_sender,
            self.state.clear_stream,
            self.state.take_stream,
        );
        let event_processor =
            spawn_event_processor(self.common.pool.clone(), self.state.event_receiver);
        let position_checker = spawn_periodic_accumulated_position_check(
            self.common.broker.clone(),
            self.common.env.clone(),
            self.common.pool.clone(),
        );
        let queue_processor = spawn_queue_processor(
            self.common.broker,
            &self.common.env,
            &self.common.pool,
            &self.common.cache,
            self.common.provider,
        );

        Conductor {
            broker_maintenance,
            order_poller,
            dex_event_receiver,
            event_processor,
            position_checker,
            queue_processor,
        }
    }
}
