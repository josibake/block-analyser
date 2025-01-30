use bitcoinkernel::{ChainType, Context, ContextBuilder, KernelError, Log, Logger};
use env_logger::Builder;
use log::LevelFilter;
use std::sync::Arc;

pub struct MainLog {}

impl Log for MainLog {
    fn log(&self, message: &str) {
        log::info!(
            target: "libbitcoinkernel", 
            "{}", message.strip_suffix("\r\n").or_else(|| message.strip_suffix('\n')).unwrap_or(message));
    }
}

pub fn setup_logging() -> Result<Logger<MainLog>, KernelError> {
    let mut builder = Builder::from_default_env();
    builder.filter(None, LevelFilter::Info).init();
    Logger::new(MainLog {})
}

pub fn create_context(chain_type: ChainType) -> Arc<Context> {
    Arc::new(
        ContextBuilder::new()
            .chain_type(chain_type)
            .build()
            .unwrap(),
    )
}
