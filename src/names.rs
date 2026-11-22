//! Midstate's `.mds` names (MSN-1), for paying MDW to `nick.mds`.
//!
//! A name's records live in midstate's name registry: one state thread whose
//! commitment is the root of a depth-24 Merkle tree of name leaves. A
//! Midstate node answers `GET /name/:name/proof` with the name's leaf, the
//! records and terms it commits to, and its Merkle proof to the root of the
//! registry's current thread. This module checks that answer instead of
//! taking the node's word for it:
//!
//! * the leaf's key is the name's (`H("MSN1" ‖ name)[..8]`), so the leaf is
//!   this name's and not another's;
//! * the records hash to the leaf's `data`, so they are the ones the leaf
//!   commits to;
//! * the proof folds the leaf up to the root;
//! * the root is the commitment of the coin the answer names, at the
//!   registry covenant's address ([`REGISTRY_ADDRESS`]), so it is a registry
//!   thread's root and not a number made up for the occasion;
//! * the name is held, and not lapsing within a day (midstate wallets refuse
//!   those too).
//!
//! Then [`prove_coin`] checks the coin is real and unspent: the Midstate node
//! proves it in its UTXO set (`GET /utxo_proof/:coin`), and the proof's
//! roots must be the state root of the midstate header at that height as
//! midwimble consensus relayed it (`core::relay`): the heaviest midstate
//! chain, by proof of work. A root a node made up, or a coin on a private
//! fork, fails there. When the relay has not reached the node's tip yet, the
//! proof is checked against the node's own roots only, and says so.
//!
//! What no single answer can show is that the coin is the registry's *own*
//! thread. Anyone can put a look-alike thread at the same address, and which
//! one is the registry is a matter of lineage since midstate's block 371,500,
//! known to a node that followed it. With your own Midstate node (the
//! default) that is your node's view; with someone else's, a lying node would
//! have to publish a look-alike thread on midstate's real chain, paying for
//! it, and keep it unspent, rather than just print a different address.

use crate::core::bond::MidstateRoots;
use crate::core::mmr::{verify_utxo_proof, UtxoProof};
use crate::core::types::{hash, hash_concat};
use crate::rpc::RpcClient;
use anyhow::{anyhow, bail, ensure, Context, Result};

pub const SUFFIX: &str = ".mds";
pub const MAX_NAME_LEN: usize = 32;
/// The registry covenant's address on midstate (its `core::names::address()`).
pub const REGISTRY_ADDRESS: &str = "c71ab5f9589e9ef822e950cce14fa0e40952fc7103d6358b13427ed96b0b742e";
/// The registry's depth: one sibling ‖ side byte per level.
pub const DEPTH: usize = 24;
/// The MDW record's tag: a midwimble stealth address, as text.
pub const TAG_MDW: u8 = 3;
/// Midstate wallets refuse a name that lapses this soon (a day of blocks).
pub const LAPSING: u64 = 1_440;
/// Where a Midstate node's RPC usually is.
pub const DEFAULT_MIDSTATE_RPC: &str = "127.0.0.1:8545";

/// Whether a destination someone typed is a name rather than an address.
pub fn is_name(s: &str) -> bool {
    s.trim().to_ascii_lowercase().ends_with(SUFFIX)
}

/// The canonical form of a name: `a-z`, `0-9`, `-`, 1 to 32 characters, no
/// `-` at either end; case and a trailing `.mds` ignored. As midstate has it.
pub fn normalize(input: &str) -> Result<String> {
    let lower = input.trim().to_ascii_lowercase();
    let s = lower.strip_suffix(SUFFIX).unwrap_or(&lower);
    ensure!(!s.is_empty() && s.len() <= MAX_NAME_LEN, "a name has 1 to {MAX_NAME_LEN} characters");
    ensure!(s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "a name uses only a-z, 0-9 and -");
    ensure!(!s.starts_with('-') && !s.ends_with('-'), "a name cannot start or end with -");
    Ok(s.to_string())
}

/// A name's key in the registry: `H("MSN1" ‖ name)[..8]`.
pub fn key(name: &str) -> [u8; 8] {
    hash(&[b"MSN1".as_slice(), name.as_bytes()].concat())[..8].try_into().unwrap()
}

/// The records' MDW address, if they give one.
pub fn mdw_record(records: &[u8]) -> Result<Option<String>> {
    let mut i = 0;
    let mut found = None;
    while i < records.len() {
        let tag = records[i];
        let len = *records.get(i + 1).ok_or_else(|| anyhow!("the records are cut short"))? as usize;
        let v = records.get(i + 2..i + 2 + len).ok_or_else(|| anyhow!("the records are cut short"))?;
        if tag == TAG_MDW && found.is_none() && v.is_ascii() {
            found = Some(String::from_utf8(v.to_vec())?);
        }
        i += 2 + len;
    }
    Ok(found)
}

/// A name's MDW address, checked against the registry's root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub name: String,
    pub address: String,
    /// The midstate height the answer is as of.
    pub height: u64,
    /// The registry root the answer was checked against, and its coin.
    pub root: [u8; 32],
    pub coin: [u8; 32],
}

fn b<const N: usize>(v: &serde_json::Value, what: &str) -> Result<[u8; N]> {
    let s = v.as_str().ok_or_else(|| anyhow!("the answer has no {what}"))?;
    hex::decode(s).ok().and_then(|x| x.try_into().ok()).ok_or_else(|| anyhow!("the answer's {what} is not {N} bytes of hex"))
}

/// Checks a node's `GET /name/:name/proof` answer for `name` (canonical) and
/// returns the MDW address its records give.
pub fn check(name: &str, v: &serde_json::Value) -> Result<Resolved> {
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        bail!("the Midstate node says: {e}");
    }
    let leaf = &v["leaf"];
    let (lkey, next) = (b::<8>(&leaf["key"], "leaf key")?, b::<8>(&leaf["next"], "leaf next")?);
    let want = key(name);
    if v["registered"].as_bool() != Some(true) {
        // The predecessor's link skips over the key: nothing is registered there.
        let (k, lo, hi) = (u64::from_le_bytes(want), u64::from_le_bytes(lkey), u64::from_le_bytes(next));
        ensure!(lo < k && k < hi, "the answer says {name}.mds is not registered, but its proof does not show that");
        bail!("{name}.mds is not registered");
    }
    ensure!(lkey == want, "the answer's leaf is another name's");
    let records = hex::decode(v["records"].as_str().unwrap_or("")).context("records")?;
    let terms = hex::decode(v["terms"].as_str().unwrap_or("")).context("terms")?;
    let (data, deal) = (b::<32>(&leaf["data"], "leaf data")?, b::<32>(&leaf["deal"], "leaf deal")?);
    ensure!(hash(&records) == data, "the records are not the ones the leaf commits to");
    ensure!(hash(&terms) == deal, "the terms are not the ones the leaf commits to");
    let expires = leaf["expires"].as_u64().ok_or_else(|| anyhow!("the answer has no expiry"))?;
    let tier = leaf["tier"].as_u64().filter(|t| *t <= 3).ok_or_else(|| anyhow!("the answer has no tier"))? as u8;
    let owner = b::<32>(&leaf["owner"], "owner")?;
    let mut bytes = Vec::with_capacity(121);
    bytes.extend_from_slice(&lkey);
    bytes.extend_from_slice(&next);
    bytes.extend_from_slice(&expires.to_le_bytes());
    bytes.push(tier);
    bytes.extend_from_slice(&owner);
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(&deal);
    let proof = hex::decode(v["proof"].as_str().unwrap_or("")).context("proof")?;
    ensure!(proof.len() == DEPTH * 33, "the proof is not {DEPTH} levels");
    let mut acc = hash(&bytes);
    for l in 0..DEPTH {
        let sib = &proof[l * 33..l * 33 + 32];
        acc = if proof[l * 33 + 32] & 1 == 1 { hash_concat(sib, &acc) } else { hash_concat(&acc, sib) };
    }
    let root = b::<32>(&v["root"], "root")?;
    ensure!(acc == root, "the proof does not reach the registry root the node gives");
    let (coin, salt) = (b::<32>(&v["thread"]["coin"], "thread coin")?, b::<32>(&v["thread"]["salt"], "thread salt")?);
    let registry: [u8; 32] = hex::decode(REGISTRY_ADDRESS).unwrap().try_into().unwrap();
    let mut h = blake3::Hasher::new();
    h.update(b"CONFIDENTIAL");
    h.update(&registry);
    h.update(&root);
    h.update(&salt);
    ensure!(*h.finalize().as_bytes() == coin, "the root is not the commitment of a registry coin");
    let height = v["height"].as_u64().ok_or_else(|| anyhow!("the answer has no height"))?;
    ensure!(expires >= height, "{name}.mds has lapsed: its holder did not renew it");
    ensure!(expires >= height + LAPSING, "{name}.mds lapses within a day; ask its holder for an address instead");
    let address = mdw_record(&records)?.ok_or_else(|| anyhow!("{name}.mds takes no MDW"))?;
    Ok(Resolved { name: name.to_string(), address, height, root, coin })
}

/// How far the registry coin behind an answer was proven on midstate's chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proven {
    /// Unspent at `height`, in the state a midstate header midwimble
    /// consensus relayed commits to, `buried` headers deep in the relay.
    Relayed { height: u64, buried: u64 },
    /// Unspent by the Midstate node's proof at `height`, against roots the
    /// relay cannot vouch for yet: it stands at `relay_tip`.
    Unrelayed { height: u64, relay_tip: u64 },
    /// The node's state at `height` is not the state of the header the relay
    /// holds there: the node follows another chain, or lies. Pay nothing on
    /// its word.
    OtherChain { height: u64 },
}

impl Proven {
    /// For people.
    pub fn describe(&self) -> String {
        match self {
            Proven::Relayed { height, buried } => format!(
                "the registry coin is unspent at midstate block {height}, in the state of the header midwimble relayed there ({buried} headers deep)"
            ),
            Proven::Unrelayed { height, relay_tip } => format!(
                "the Midstate node proves the registry coin unspent at block {height}; the relay stands at {relay_tip}, so that block's state is the node's word for now"
            ),
            Proven::OtherChain { height } => format!(
                "the Midstate node's state at block {height} is not the state of the header midwimble relayed there: it follows another chain"
            ),
        }
    }
}

/// Checks a Midstate node's `GET /utxo_proof/:coin` answer `v` for `coin`:
/// the coin is in the UTXO set the answer's roots commit to, and those roots
/// are the state root of the header at the answer's height that `relayed`
/// gives, when it gives one (`None`: the relay does not hold that height).
pub fn check_coin(coin: &[u8; 32], v: &serde_json::Value, relayed: impl Fn(u64) -> Option<[u8; 32]>, relay_tip: u64) -> Result<Proven> {
    use crate::core::auxpow::bytes32;
    if let Some(e) = v.get("error") {
        bail!("the Midstate node cannot prove the registry coin unspent ({e}): the registry may have just moved on, so try again");
    }
    let roots = MidstateRoots {
        coins: bytes32(&v["coins_root"])?,
        commitments: bytes32(&v["commitments_root"])?,
        chain_mmr: bytes32(&v["chain_mmr_root"])?,
        burned_wots: match &v["burned_wots_root"] {
            serde_json::Value::Null => None,
            b => Some(bytes32(b)?),
        },
    };
    let height = v["height"].as_u64().ok_or_else(|| anyhow!("the coin proof has no height"))?;
    let smt: UtxoProof = serde_json::from_value(v["proof"].clone()).context("the coin proof")?;
    ensure!(verify_utxo_proof(coin, &smt, &roots.coins, true), "the Midstate node's proof does not show the registry coin unspent");
    let root = roots.state_root(height)?;
    match relayed(height) {
        Some(r) if r != root => Ok(Proven::OtherChain { height }),
        Some(_) => Ok(Proven::Relayed { height, buried: relay_tip.saturating_sub(height) }),
        None => Ok(Proven::Unrelayed { height, relay_tip }),
    }
}

/// Asks the Midstate node at `midstate` to prove `coin` unspent, and checks
/// the proof against the midwimble node at `midwimble`'s relay
/// (`GET /relay/{height}`) as [`check_coin`] says.
pub fn prove_coin(midstate: &RpcClient, midwimble: &RpcClient, coin: &[u8; 32]) -> Result<Proven> {
    let v = midstate.get(&format!("/utxo_proof/{}", hex::encode(coin))).context("asking the Midstate node to prove the registry coin unspent")?;
    let height = v["height"].as_u64().unwrap_or(0);
    let entry = midwimble.get(&format!("/relay/{height}")).ok();
    let relay_tip = entry.as_ref().and_then(|e| e["tip_height"].as_u64()).or_else(|| midwimble.get("/relay").ok()?["tip_height"].as_u64()).unwrap_or(0);
    let root = entry.filter(|e| e["height"].as_u64() == Some(height)).and_then(|e| b::<32>(&e["state_root"], "state root").ok());
    check_coin(coin, &v, |h| root.filter(|_| h == height), relay_tip)
}

/// Resolves `name` to its MDW address through the Midstate node at `midstate`
/// (`host:port`), checking the answer as [`check`] says.
pub fn resolve(midstate: &RpcClient, name: &str) -> Result<Resolved> {
    let name = normalize(name)?;
    let v = midstate
        .get(&format!("/name/{name}/proof"))
        .with_context(|| format!("asking the Midstate node for {name}.mds (is it running, and v6.6 or later?)"))?;
    let r = check(&name, &v)?;
    crate::core::mw::StealthAddress::decode(&r.address)
        .with_context(|| format!("{name}.mds gives an MDW address that does not decode"))?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-leaf registry around `records`: the answer a node would give.
    fn answer(name: &str, records: &[u8], expires: u64, height: u64) -> serde_json::Value {
        let k = key(name);
        let next = [0xffu8; 8];
        let data = hash(records);
        let terms = [0u8];
        let deal = hash(&terms);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&k);
        bytes.extend_from_slice(&next);
        bytes.extend_from_slice(&expires.to_le_bytes());
        bytes.push(3);
        bytes.extend_from_slice(&[7; 32]);
        bytes.extend_from_slice(&data);
        bytes.extend_from_slice(&deal);
        // Slot 1, empty siblings except the sentinel's leaf at slot 0.
        let mut proof = Vec::new();
        let mut acc = hash(&bytes);
        let mut empty = [0u8; 32];
        for l in 0..DEPTH {
            let sib = if l == 0 { [5u8; 32] } else { empty };
            let side = if l == 0 { 1 } else { 0 };
            proof.extend_from_slice(&sib);
            proof.push(side);
            acc = if side == 1 { hash_concat(&sib, &acc) } else { hash_concat(&acc, &sib) };
            empty = hash_concat(&empty, &empty);
        }
        let salt = [9u8; 32];
        let registry: [u8; 32] = hex::decode(REGISTRY_ADDRESS).unwrap().try_into().unwrap();
        let mut h = blake3::Hasher::new();
        h.update(b"CONFIDENTIAL");
        h.update(&registry);
        h.update(&acc);
        h.update(&salt);
        serde_json::json!({
            "registered": true,
            "leaf": { "key": hex::encode(k), "next": hex::encode(next), "expires": expires, "tier": 3, "owner": hex::encode([7; 32]), "data": hex::encode(data), "deal": hex::encode(deal) },
            "records": hex::encode(records),
            "terms": hex::encode(terms),
            "proof": hex::encode(&proof),
            "root": hex::encode(acc),
            "thread": { "coin": hex::encode(h.finalize().as_bytes()), "salt": hex::encode(salt) },
            "height": height,
        })
    }

    fn records(mdw: &str) -> Vec<u8> {
        let mut r = vec![1, 33];
        r.extend_from_slice(&[4; 32]);
        r.push(10);
        r.push(3);
        r.push(mdw.len() as u8);
        r.extend_from_slice(mdw.as_bytes());
        r
    }

    #[test]
    fn a_checked_answer_gives_the_names_mdw_address() {
        let a = answer("nick", &records("mw1abc"), 10_000, 5_000);
        let r = check("nick", &a).unwrap();
        assert_eq!(r.address, "mw1abc");
        assert_eq!(r.height, 5_000);
    }

    #[test]
    fn a_node_that_lies_is_caught() {
        let a = answer("nick", &records("mw1abc"), 10_000, 5_000);
        // Other records than the leaf's.
        let mut forged = a.clone();
        forged["records"] = hex::encode(records("mw1evil")).into();
        assert!(check("nick", &forged).unwrap_err().to_string().contains("not the ones the leaf commits to"));
        // The leaf rewritten to match: then it no longer reaches the root.
        let other = answer("nick", &records("mw1evil"), 10_000, 5_000);
        let mut moved = other.clone();
        moved["root"] = a["root"].clone();
        moved["thread"] = a["thread"].clone();
        assert!(check("nick", &moved).unwrap_err().to_string().contains("does not reach"));
        // A root that is no registry coin's.
        let mut coinless = a.clone();
        coinless["thread"]["salt"] = hex::encode([1u8; 32]).into();
        assert!(check("nick", &coinless).unwrap_err().to_string().contains("not the commitment of a registry coin"));
        // Another name's leaf.
        assert!(check("nicky", &a).unwrap_err().to_string().contains("another name's"));
    }

    #[test]
    fn lapsed_names_and_names_without_mdw_are_refused() {
        assert!(check("nick", &answer("nick", &records("mw1abc"), 4_999, 5_000)).unwrap_err().to_string().contains("lapsed"));
        assert!(check("nick", &answer("nick", &records("mw1abc"), 5_100, 5_000)).unwrap_err().to_string().contains("within a day"));
        let no_mdw = vec![6, 2, b'h', b'i'];
        assert!(check("nick", &answer("nick", &no_mdw, 10_000, 5_000)).unwrap_err().to_string().contains("takes no MDW"));
    }

    /// An answer a real Midstate node's name index gave (midstate's
    /// `storage::name_index` test chain, `mdwname` with MDW record
    /// `mw1fixture`), so the two codebases are checked against each other.
    #[test]
    fn a_real_midstate_answer_checks_out() {
        let v: serde_json::Value = serde_json::from_str(include_str!("../tests/fixtures/midstate_name_proof.json")).unwrap();
        assert_eq!(v["registry"], REGISTRY_ADDRESS, "the registry address is pinned on both sides");
        let r = check("mdwname", &v).unwrap();
        assert_eq!(r.address, "mw1fixture");
        let mut forged = v.clone();
        forged["records"] = hex::encode(records("mw1evil")).into();
        assert!(check("mdwname", &forged).is_err());
    }

    /// A Midstate node's `GET /utxo_proof/:coin` answer for `coin` in a
    /// small UTXO set at `height`, and the header state root it implies.
    fn coin_answer(coin: &[u8; 32], height: u64) -> (serde_json::Value, [u8; 32]) {
        let mut coins = crate::core::mmr::UtxoAccumulator::new();
        for i in 0..20u8 {
            coins.insert(hash(&[i]), true);
        }
        coins.insert(*coin, true);
        let roots = MidstateRoots {
            coins: coins.root(true),
            commitments: hash(b"commitments"),
            chain_mmr: hash(b"mmr"),
            burned_wots: Some(hash(b"burned")),
        };
        let v = serde_json::json!({
            "height": height,
            "coins_root": hex::encode(roots.coins),
            "commitments_root": hex::encode(roots.commitments),
            "chain_mmr_root": hex::encode(roots.chain_mmr),
            "burned_wots_root": hex::encode(roots.burned_wots.unwrap()),
            "proof": serde_json::to_value(coins.prove(coin, true).unwrap()).unwrap(),
        });
        (v, roots.state_root(height).unwrap())
    }

    #[test]
    fn the_registry_coin_is_proven_against_the_relayed_header() {
        let coin = hash(b"registry thread");
        let h = 600_000;
        let (v, header_root) = coin_answer(&coin, h);
        // The relay holds that header: proven, and how deep.
        let p = check_coin(&coin, &v, |x| (x == h).then_some(header_root), h + 12).unwrap();
        assert_eq!(p, Proven::Relayed { height: h, buried: 12 });
        // Not yet: the node's word for that block's state, and it says so.
        assert_eq!(check_coin(&coin, &v, |_| None, h - 3).unwrap(), Proven::Unrelayed { height: h, relay_tip: h - 3 });
        // Another chain's state at that height: caught.
        assert_eq!(check_coin(&coin, &v, |_| Some([9; 32]), h).unwrap(), Proven::OtherChain { height: h });
        // A proof for another coin, or roots that do not hold it: refused.
        assert!(check_coin(&hash(b"look-alike"), &v, |_| Some(header_root), h).is_err());
        let mut moved = v.clone();
        moved["coins_root"] = hex::encode([1u8; 32]).into();
        assert!(check_coin(&coin, &moved, |_| Some(header_root), h).is_err());
        // The node could not prove it (the thread just moved on).
        assert!(check_coin(&coin, &serde_json::json!({ "error": "coin not found" }), |_| None, h).unwrap_err().to_string().contains("try again"));
    }

    #[test]
    fn names_normalize_as_midstate_has_them() {
        assert_eq!(normalize(" Nick.MDS ").unwrap(), "nick");
        for bad in ["", ".mds", "-a", "a-", "a b", &"x".repeat(33)] {
            assert!(normalize(bad).is_err(), "{bad:?}");
        }
        assert!(is_name("nick.mds") && !is_name("mw1abc"));
        // Midstate's key for "nick", as its name index stores it.
        assert_eq!(key("nick"), hash(b"MSN1nick")[..8]);
    }
}
