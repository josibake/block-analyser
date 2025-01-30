use bitcoinkernel::{
    BlockManagerOptions, ChainType, ChainstateLoadOptions, ChainstateManager,
    ChainstateManagerOptions, ScriptPubkey,
};
use clap::Parser;
use log::{error, info, warn};
use rayon::prelude::*;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

mod kernel;
use crate::kernel::{create_context, setup_logging};

#[derive(Debug, Clone)]
struct BlockResult {
    height: i32,
    total_txs: u32,
    total_inputs: u32,
    mixed_tx_count: u32,
    schnorr_sigs: u32,
    non_schnorr_sigs: u32,
}

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
    /// Output CSV file
    #[arg(long, default_value = "block_stats.csv")]
    output: String,
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

fn write_results_to_csv(results: &[BlockResult], filename: &str) -> std::io::Result<()> {
    info!("Writing results to CSV file: {}", filename);
    let start_time = Instant::now();

    let mut file = File::create(filename)?;

    // Write header
    writeln!(
        file,
        "height,total_txs,total_inputs,mixed_tx_count,schnorr_sigs,non_schnorr_sigs"
    )?;

    // Write data
    for result in results {
        writeln!(
            file,
            "{},{},{},{},{},{}",
            result.height,
            result.total_txs,
            result.total_inputs,
            result.mixed_tx_count,
            result.schnorr_sigs,
            result.non_schnorr_sigs
        )?;
    }

    info!(
        "CSV file written successfully in {:.2}s",
        start_time.elapsed().as_secs_f32()
    );
    Ok(())
}

fn process_blocks(chainman: &ChainstateManager, start: i32, end: i32) -> Vec<BlockResult> {
    info!("Starting block processing from height {} to {}", start, end);
    let start_time = Instant::now();

    // Create a vector of block heights to process
    let block_heights: Vec<i32> = (start..=end).collect();
    let results = Arc::new(Mutex::new(Vec::new()));
    let progress = Arc::new(Mutex::new((0, block_heights.len())));

    // Process blocks in parallel
    block_heights.par_iter().for_each(|height| {
        let mut mixed_tx_count = 0;
        let mut schnorr_count = 0;
        let mut non_schnorr_count = 0;
        let mut total_txs = 0;
        let mut total_inputs = 0;

        if let Ok(block_index) = chainman.get_block_index_by_height(*height) {
            if let Ok(undo) = chainman.read_undo_data(&block_index) {
                // Process each transaction
                total_txs = undo.n_tx_undo;
                for i in 0..undo.n_tx_undo {
                    let mut has_schnorr = false;
                    let mut has_non_schnorr = false;

                    let transaction_undo_size =
                        undo.get_transaction_undo_size(i.try_into().unwrap());
                    total_inputs += transaction_undo_size;
                    // Process each prevout
                    for j in 0..transaction_undo_size {
                        if let Ok(prevout) =
                            undo.get_prevout_by_index(i.try_into().unwrap(), j.try_into().unwrap())
                        {
                            if is_p2tr(prevout.get_script_pubkey()) {
                                has_schnorr = true;
                                schnorr_count += 1;
                            } else {
                                has_non_schnorr = true;
                                non_schnorr_count += 1;
                            }
                        }
                    }

                    if has_schnorr && has_non_schnorr {
                        mixed_tx_count += 1;
                    }
                }
            } else {
                warn!("Failed to read undo data for block at height {}", height);
            }

            // Store the results for this block
            if let Ok(mut results_guard) = results.lock() {
                results_guard.push(BlockResult {
                    height: *height,
                    total_txs: total_txs as u32,
                    total_inputs: total_inputs as u32,
                    mixed_tx_count,
                    schnorr_sigs: schnorr_count,
                    non_schnorr_sigs: non_schnorr_count,
                });
            }

            // Update and log progress
            if let Ok(mut progress_guard) = progress.lock() {
                progress_guard.0 += 1;
                if progress_guard.0 % 10000 == 0 || progress_guard.0 == progress_guard.1 {
                    info!(
                        "Processed {}/{} blocks ({:.1}%) in {:.2}s",
                        progress_guard.0,
                        progress_guard.1,
                        (progress_guard.0 as f32 / progress_guard.1 as f32) * 100.0,
                        start_time.elapsed().as_secs_f32()
                    );
                }
            }
        } else {
            warn!("Failed to get block index for height {}", height);
        }
    });

    // Sort results by height and return
    let mut final_results = results.lock().unwrap().to_vec();
    final_results.sort_by_key(|r| r.height);

    info!(
        "Block processing completed in {:.2}s. Analyzed {} blocks.",
        start_time.elapsed().as_secs_f32(),
        final_results.len()
    );

    final_results
}

fn main() {
    let args = Args::parse();
    info!(
        "Starting analysis with network: {}, data directory: {}",
        args.network, args.datadir
    );

    let chain_type = match args.network.to_lowercase().as_str() {
        "mainnet" => ChainType::MAINNET,
        "testnet" => ChainType::TESTNET,
        "regtest" => ChainType::REGTEST,
        "signet" => ChainType::SIGNET,
        _ => {
            error!("Invalid network type: {}", args.network);
            std::process::exit(1);
        }
    };
    let data_dir = args.datadir;
    let blocks_dir = data_dir.clone() + "/blocks";

    // Set up the kernel
    info!("Setting up bitcoinkernel...");
    let start_time = Instant::now();
    let _ = setup_logging().unwrap();
    let context = create_context(chain_type);
    let chainman = ChainstateManager::new(
        ChainstateManagerOptions::new(&context, &data_dir).unwrap(),
        BlockManagerOptions::new(&context, &blocks_dir).unwrap(),
        ChainstateLoadOptions::new(),
        Arc::clone(&context),
    )
    .unwrap();

    info!("Importing blocks...");
    chainman.import_blocks().unwrap();
    info!(
        "Setup completed in {:.2}s",
        start_time.elapsed().as_secs_f32()
    );

    // Process blocks with the specified range and collect results
    let results = process_blocks(&chainman, args.start, args.end);

    // Write results to CSV
    if let Err(e) = write_results_to_csv(&results, &args.output) {
        error!("Error writing CSV file: {}", e);
        std::process::exit(1);
    }

    info!("Analysis complete. Results written to {}", args.output);
}
