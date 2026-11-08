//! Merged mining across both switches to Midstate Walk, with CPU stand-ins
//! for CUDA GPUs: midwimble switches first (37,300 on mainnet, 20 here),
//! merged mining pauses while midstate still takes the BLAKE3 chain, and
//! resumes on the GPUs once midstate switches too (371,500 on mainnet,
//! `MIDSTATE_WALK_ACTIVATION_HEIGHT` here).
//!
//! `cargo test --features fast-mining,gpu --test cuda_walk_merge`

use midwimble::core::cuda_walk::{install_for_tests, WalkBatcher};
use midwimble::core::extension::create_extension_with;
use midwimble::core::gpu_mining::{set_backend, Backend};
use midwimble::core::mw::WalletKeys;
use midwimble::core::types::{
    midstate_pow_algo_at, pow_algo_at, PowAlgo, MIDSTATE_WALK_ACTIVATION_HEIGHT,
    WALK_ACTIVATION_HEIGHT,
};
use midwimble::core::walk;
use midwimble::merge_mine::{run, MergeMineConfig, MergeStats};
use midwimble::node::{Node, NodeConfig, NodeHandle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod support;
use support::*;

const GPU_BIT: u64 = 1 << 63;

/// A GPU that hashes on the CPU, correctly.
struct StandIn;
impl WalkBatcher for StandIn {
    fn label(&self) -> String {
        "stand-in gpu".into()
    }
    fn capacity(&self) -> usize {
        16
    }
    fn hash_batch(
        &mut self,
        h: &[u8; 32],
        nonces: &[u64],
        cancel: &AtomicBool,
    ) -> Result<Option<Vec<[u8; 32]>>, String> {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        Ok(Some(nonces.iter().map(|n| walk::walk_hash(h, *n)).collect()))
    }
}

async fn start_midwimble() -> (NodeHandle, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = NodeConfig::new(dir.path(), "/ip4/127.0.0.1/tcp/0".parse().unwrap());
    config.poll_interval = Duration::from_millis(500);
    let (node, handle) = Node::new(config).await.unwrap();
    tokio::spawn(node.run());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc = listener.local_addr().unwrap().to_string();
    tokio::spawn(midwimble::rpc::serve_on(handle.clone(), listener));
    (handle, rpc, dir)
}

async fn wait_until(what: &str, secs: u64, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn merged_mining_pauses_between_the_switches_and_resumes_on_the_gpus() {
    // GPU-only for the walk, so every walk result here came from a GPU.
    // The BLAKE3 chain, with no usable wgpu device, goes to the CPU.
    install_for_tests(vec![Box::new(StandIn), Box::new(StandIn)]);
    set_backend(Backend::Gpu);

    let (midstate_rpc, mock) = start_mock_midstate().await;
    let (mw, mw_rpc, _dir) = start_midwimble().await;
    let log_dir = tempfile::tempdir().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(MergeStats::default());
    let cfg = MergeMineConfig {
        midstate_rpc,
        midwimble_rpc: mw_rpc,
        midstate_address: [7; 32],
        midwimble_address: WalletKeys::random().address(),
        threads: 1,
        coinbase_log: log_dir.path().join("coinbase.jsonl"),
        refresh: Duration::from_secs(5),
        anchor_store: None,
        anchor_depth: 2,
    };
    let miner = {
        let (stop, stats) = (stop.clone(), stats.clone());
        std::thread::spawn(move || run(cfg, stop, stats))
    };

    // 1. Both on the BLAKE3 chain: merged, until midwimble's next block is
    //    its first walk block.
    wait_until("midwimble to reach its switch", 600, || {
        mw.state().height >= WALK_ACTIVATION_HEIGHT
    })
    .await;
    assert_eq!(mw.state().height, WALK_ACTIVATION_HEIGHT, "no walk block yet: midstate has not switched");
    let paused_from = mock.lock().unwrap().accepted;
    assert_eq!(
        midstate_pow_algo_at(mock.lock().unwrap().height),
        PowAlgo::Blake3Chain
    );

    // 2. The pause: midstate is mined alone, midwimble waits.
    wait_until("midstate blocks during the pause", 120, || {
        mock.lock().unwrap().accepted >= paused_from + 3
    })
    .await;
    assert_eq!(mw.state().height, WALK_ACTIVATION_HEIGHT, "midwimble waits out the pause");

    // Midstate's chain moves on to a few blocks before its own switch (the
    // stand-in serves whatever height it is told; a real chain gets there
    // by mining). Both sides of the jump take the BLAKE3 chain.
    {
        let mut c = mock.lock().unwrap();
        assert!(c.height < MIDSTATE_WALK_ACTIVATION_HEIGHT - 2);
        c.height = MIDSTATE_WALK_ACTIVATION_HEIGHT - 2;
    }

    // 3. Midstate switches: both on the walk, merged again, on the GPUs.
    wait_until("merged walk blocks", 300, || {
        mw.state().height >= WALK_ACTIVATION_HEIGHT + 3
    })
    .await;
    stop.store(true, Ordering::Relaxed);
    miner.join().unwrap().unwrap();
    assert_eq!(stats.rejected.load(Ordering::Relaxed), 0, "nothing either chain refused");

    // Every midwimble block is merged-mined, with the proof of work its
    // height takes; from the switch on, found by a GPU.
    let state = mw.state();
    for h in 1..state.height {
        let b = mw.storage().load_batch(h).unwrap().unwrap();
        let aux = b.aux_pow.as_ref().unwrap_or_else(|| panic!("block {h} was merge-mined"));
        if pow_algo_at(h) == PowAlgo::Walk {
            assert!(aux.nonce & GPU_BIT != 0, "block {h}: the walk, on a GPU");
        } else {
            assert!(aux.nonce & GPU_BIT == 0, "block {h}: the BLAKE3 chain, not the CUDA walker");
        }
    }
    state.verify_supply().unwrap();

    // Midstate's blocks: the BLAKE3 chain up to its switch, including the
    // pause, and the walk from it, each as the stand-in node checked.
    let c = mock.lock().unwrap();
    let mut walk_blocks = 0;
    for (i, b) in c.blocks.iter().enumerate() {
        let gpu = b.extension.nonce & GPU_BIT != 0;
        let walk = create_extension_with(PowAlgo::Walk, b.mining_hash().0, b.extension.nonce).final_hash
            == b.extension.final_hash;
        assert_eq!(gpu, walk, "midstate block {i}: GPU nonces are walk results, and only those");
        walk_blocks += walk as usize;
    }
    assert!(walk_blocks >= 3, "midstate's walk blocks: {walk_blocks}");
}
