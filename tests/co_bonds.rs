//! Co-bonds handed in to a running producer (`co_bonds`, `/co-bonds`).
//!
//! `cargo test --features fast-mining --test co_bonds`
//!
//! A pool shows its mining key from `GET /co-bonds` and hands in the bonds
//! people lock on it with `POST /co-bonds`. The node takes only a bond on its
//! own key whose coin is on midstate; the registration that follows is
//! `midwimble bond register`'s, run by the node.
#![cfg(feature = "fast-mining")]

use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
use midwimble::core::bond::{
    devnet_registration, est_midstate_height, BondScript, MinerBond, MAX_MINING_BOND,
    MIN_REMAINING_BOND_LOCK,
};
use midwimble::core::state::current_timestamp;
use midwimble::core::types::{hash, GENESIS_TARGET};
use midwimble::node::{Node, NodeConfig};
use midwimble::rpc::RpcClient;
use serde_json::json;

mod support;
use support::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_producer_shows_its_key_and_takes_only_bonds_it_can_register() {
    let dir = tempfile::tempdir().unwrap();
    let secret = Scalar::from_bytes_mod_order(hash(b"co-bond producer"));
    let mining_key = RistrettoPoint::mul_base(&secret).compress().to_bytes();
    let until = est_midstate_height(current_timestamp()) + MIN_REMAINING_BOND_LOCK + 10_000;
    let (registration, headers) = devnet_registration(mining_key, until, hash(b"own salt"), &GENESIS_TARGET);
    let bond = MinerBond {
        secret,
        bond_id: registration.bond_id(),
        registration: Some(registration),
        midstate: Some(headers),
        co_bonds: vec![],
    };
    let bond_id = hex::encode(bond.bond_id);
    let (midstate_rpc, _mock) = start_mock_midstate().await;
    let mut config = NodeConfig::new(dir.path(), "/ip4/127.0.0.1/tcp/0".parse().unwrap());
    config.mining_bond = Some(bond);
    config.midstate_rpc = Some(midstate_rpc);
    let (node, handle) = Node::new(config).await.unwrap();
    let _node = tokio::spawn(node.run());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc = listener.local_addr().unwrap().to_string();
    tokio::spawn(midwimble::rpc::serve_on(handle.clone(), listener));

    tokio::task::spawn_blocking(move || {
        let mw = RpcClient::new(rpc);

        // What a pool shows: its mining key and its own bond, no co-bonds yet.
        let listed = mw.get("/co-bonds").expect("GET /co-bonds");
        assert_eq!(listed["mining_key"], hex::encode(mining_key));
        assert_eq!(listed["bond_id"], bond_id);
        assert_eq!(listed["co_bonds"].as_array().map(Vec::len), Some(0));

        let coin = |key: [u8; 32]| {
            let script = BondScript { mining_key: key, bonded_until: until, owner_pk: [7; 32] };
            json!({
                "script": hex::encode(script.to_bytecode()),
                "value": MAX_MINING_BOND,
                "salt": hex::encode([9u8; 32]),
            })
        };
        // A bond on another key is no co-bond of this node's.
        let (status, v) = mw.post_raw("/co-bonds", &coin([3; 32])).unwrap();
        assert_eq!(status, 400);
        assert!(v["error"].as_str().unwrap().contains("another mining key"), "{v}");
        // Nor is something that is not a bond script at all.
        let (status, v) = mw
            .post_raw("/co-bonds", &json!({ "script": "00", "value": 1, "salt": hex::encode([9u8; 32]) }))
            .unwrap();
        assert_eq!(status, 400, "{v}");
        // On this node's key, but no such coin on midstate: refused, so
        // nobody fills the book with coins that are not there.
        let (status, v) = mw.post_raw("/co-bonds", &coin(mining_key)).unwrap();
        assert_eq!(status, 400);
        assert!(v["error"].as_str().unwrap().contains("no such coin"), "{v}");

        let listed = mw.get("/co-bonds").unwrap();
        assert_eq!(listed["co_bonds"].as_array().map(Vec::len), Some(0), "nothing was taken");
    })
    .await
    .unwrap();
    // The handle answers the same way as the RPC.
    let book = handle.co_bonds().expect("a node with a bond takes co-bonds");
    assert_eq!(book.mining_key(), hex::encode(mining_key));
    assert_eq!(handle.mining_bond().map(|b| b.co_bonds.len()), Some(0));
}
