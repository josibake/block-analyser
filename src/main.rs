use bitcoinkernel::{
    BlockManagerOptions, ChainType, ChainstateLoadOptions, ChainstateManager,
    ChainstateManagerOptions, ScriptPubkey,
};
use clap::Parser;
use rayon::prelude::*;
use std::sync::Arc;

mod kernel;
use crate::kernel::{create_context, setup_logging};

/// A simple CLI tool
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Data directory
    #[arg(long)]
    datadir: String,

    /// Network
    #[arg(long)]
    network: String,

    /// Start block height
    #[arg(long)]
    start: i32,

    /// End block height
    #[arg(long)]
    end: i32,
}

/// Check if a script public key is Pay-to-Taproot (P2TR)
fn is_p2tr(spk: ScriptPubkey) -> bool {
    let spk_bytes = spk.get();
    if spk_bytes.len() != 34 {
        return false;
    }
    // OP_1 (0x51) OP_PUSHBYTES_32 (0x20) <32 bytes>
    spk_bytes[0] == 0x51 && spk_bytes[1] == 0x20
}

fn process_blocks(chainman: &ChainstateManager, start: i32, end: i32) {
    // Create a vector of block heights to process
    let block_heights: Vec<i32> = (start..=end).collect();

    // Process blocks in parallel
    block_heights.par_iter().for_each(|height| {
        let block_index = chainman.get_block_index_by_height(*height).unwrap();
        let undo = chainman.read_undo_data(&block_index).unwrap();
        // Process each transaction in parallel
        (0..undo.n_tx_undo).into_par_iter().for_each(|i| {
            let transaction_undo_size: u64 = undo.get_transaction_undo_size(i.try_into().unwrap());
            (0..transaction_undo_size).into_par_iter().for_each(|j| {
                if is_p2tr(
                    undo.get_prevout_by_index(i as u64, j)
                        .unwrap()
                        .get_script_pubkey(),
                ) {
                    println!("found taproot");
                }
            });
        });
    });
}

fn main() {
    let args = Args::parse();
    let chain_type = match args.network.to_lowercase().as_str() {
        "mainnet" => ChainType::MAINNET,
        "testnet" => ChainType::TESTNET,
        "regtest" => ChainType::REGTEST,
        "signet" => ChainType::SIGNET,
        _ => {
            eprintln!("Invalid network type: {}", args.network);
            std::process::exit(1);
        }
    };
    let data_dir = args.datadir;
    let blocks_dir = data_dir.clone() + "/blocks";

    // Set up the kernel
    let _ = setup_logging().unwrap();
    let context = create_context(chain_type);
    let chainman = ChainstateManager::new(
        ChainstateManagerOptions::new(&context, &data_dir).unwrap(),
        BlockManagerOptions::new(&context, &blocks_dir).unwrap(),
        ChainstateLoadOptions::new(),
        Arc::clone(&context),
    )
    .unwrap();
    chainman.import_blocks().unwrap();

    // Process blocks with the specified range
    process_blocks(&chainman, args.start, args.end);
}
