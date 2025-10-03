use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::sol_types;
use futures_util::Stream;
use sqlx::SqlitePool;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tracing::info;

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

struct CommonFields<P> {
    env: Env,
    pool: SqlitePool,
    cache: SymbolCache,
    provider: P,
}

pub(crate) struct Initial;

pub(crate) struct WithToken {
    token_refresher: JoinHandle<()>,
}

pub(crate) struct WithDexStreams {
    token_refresher: JoinHandle<()>,
    clear_stream: ClearStream,
    take_stream: TakeStream,
    event_sender: UnboundedSender<(TradeEvent, Log)>,
    event_receiver: UnboundedReceiver<(TradeEvent, Log)>,
}

pub(crate) struct ConductorBuilder<P, State> {
    common: CommonFields<P>,
    state: State,
}

impl<P: Provider + Clone + Send + 'static> ConductorBuilder<P, Initial> {
    pub(crate) fn new(env: Env, pool: SqlitePool, cache: SymbolCache, provider: P) -> Self {
        Self {
            common: CommonFields {
                env,
                pool,
                cache,
                provider,
            },
            state: Initial,
        }
    }

    pub(crate) fn with_token_refresher(
        self,
        token_refresher: JoinHandle<()>,
    ) -> ConductorBuilder<P, WithToken> {
        ConductorBuilder {
            common: self.common,
            state: WithToken { token_refresher },
        }
    }
}

impl<P: Provider + Clone + Send + 'static> ConductorBuilder<P, WithToken> {
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
    ) -> ConductorBuilder<P, WithDexStreams> {
        let (event_sender, event_receiver) =
            tokio::sync::mpsc::unbounded_channel::<(TradeEvent, Log)>();

        ConductorBuilder {
            common: self.common,
            state: WithDexStreams {
                token_refresher: self.state.token_refresher,
                clear_stream: Box::new(clear_stream),
                take_stream: Box::new(take_stream),
                event_sender,
                event_receiver,
            },
        }
    }
}

impl<P: Provider + Clone + Send + 'static> ConductorBuilder<P, WithDexStreams> {
    pub(crate) fn spawn(self) -> Conductor {
        info!("Starting conductor orchestration");

        let broker = self.common.env.get_broker();
        let order_poller = spawn_order_poller(&self.common.env, &self.common.pool, broker.clone());
        let dex_event_receiver = spawn_onchain_event_receiver(
            self.state.event_sender,
            self.state.clear_stream,
            self.state.take_stream,
        );
        let event_processor =
            spawn_event_processor(self.common.pool.clone(), self.state.event_receiver);
        let position_checker = spawn_periodic_accumulated_position_check(
            broker.clone(),
            self.common.env.clone(),
            self.common.pool.clone(),
        );
        let queue_processor = spawn_queue_processor(
            broker,
            &self.common.env,
            &self.common.pool,
            &self.common.cache,
            self.common.provider,
        );

        Conductor {
            token_refresher: self.state.token_refresher,
            order_poller,
            dex_event_receiver,
            event_processor,
            position_checker,
            queue_processor,
        }
    }
}
