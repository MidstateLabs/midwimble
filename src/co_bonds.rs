//! Co-bonds a producer takes while it runs.
//!
//! A co-bond is a bond someone else locked on midstate naming this node's
//! mining key (`core::bond::CoBond`): they keep the coin, which only their
//! owner key can spend, and the node signs blocks with it. `--co-bond` loads
//! registered co-bond files at startup. This module takes them while the node
//! runs, from no more than the coin (its script, value and salt), and
//! registers them itself. It takes a proof of the coin from the midstate node
//! the relay reads (`--midstate-rpc`), waits for a day of midwimble's work
//! above it, and has this node's blocks carry it, with a fresh proof whenever
//! the relay's window is about to pass the old one. That is `midwimble bond
//! register`, run here, so nobody passes files back and forth while a proof
//! ages out.
//!
//! The book is `co-bonds.json` in the data directory. Handing in a coin grants
//! nothing but the right to sign with it, which its script already names this
//! node for. A coin that is not a bond on this node's key, not on midstate,
//! or too small or too short to count is refused.

use crate::core::auxpow::bytes32;
use crate::core::bond::{
    bond_proof_from_json, burial_headers, est_midstate_height, header_link_from_json,
    min_remaining_bond_lock, registration_cap, registration_min_bond, registration_work,
    BondCoin, BondFile, BondRegistration, CoBond, HeldProof, MinerBond, BOND_RULES,
    MAX_BOND_HORIZON,
};
use crate::core::relay::{
    window_left, MidstateHeaders, RelayFeed, MAX_RELAY_HEADERS_PER_BLOCK, RELAY_WINDOW,
};
use crate::core::state::{calculate_work, current_timestamp};
use crate::node::NodeInfo;
use crate::rpc::RpcClient;
use anyhow::{anyhow, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

/// The most co-bonds the book holds, so handing in coins cannot grow it
/// without bound.
pub const MAX_CO_BONDS: usize = 4_096;
/// How often the book looks for a step to take (seconds). Each look takes at
/// most one registration step, so a long book is worked through in turn.
pub const TICK_SECS: u64 = 30;
/// How long an expired co-bond stays listed before the book forgets it.
const KEEP_EXPIRED_SECS: u64 = 7 * 86_400;

/// One co-bond in the book.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    coin: BondCoin,
    /// The proof, registration and headers the last step left, as a co-bond
    /// file would hold them.
    #[serde(default)]
    file: Option<BondFile>,
    /// When it was handed in (unix seconds).
    added: u64,
    /// "waiting" | "burying" | "ready" | "registered" | "expired" | "failed"
    phase: String,
    #[serde(default)]
    progress: u32,
    #[serde(default)]
    note: String,
    /// When its next step is due (unix seconds).
    #[serde(default)]
    due: u64,
}

impl Entry {
    fn view(&self) -> CoBondView {
        CoBondView {
            bond_id: hex::encode(self.coin.coin_id()),
            value: self.coin.value,
            bonded_until: self.coin.script.bonded_until,
            phase: self.phase.clone(),
            progress: self.progress,
            note: self.note.clone(),
            added: self.added,
        }
    }

    /// What the node signs with for it now, if anything.
    fn signer(&self) -> Option<CoBond> {
        let bond_id = self.coin.coin_id();
        match self.phase.as_str() {
            "registered" => Some(CoBond { bond_id, registration: None, midstate: None }),
            "ready" => {
                let f = self.file.as_ref()?;
                Some(CoBond {
                    bond_id,
                    registration: f.registration.clone(),
                    midstate: f.midstate_headers.clone(),
                })
            }
            _ => None,
        }
    }
}

/// A co-bond, for the RPC.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CoBondView {
    /// The coin's id, which is the bond's (hex).
    pub bond_id: String,
    pub value: u64,
    pub bonded_until: u64,
    /// "waiting": for its proof · "burying": a day of work above it ·
    /// "ready": this node's next blocks register it · "registered": it signs
    /// · "expired": it no longer counts · "failed": the last step failed, and
    /// the next will retry.
    pub phase: String,
    pub progress: u32,
    pub note: String,
    pub added: u64,
}

/// The co-bonds this node has taken, and what its templates sign with.
pub struct CoBondBook {
    path: PathBuf,
    midstate_rpc: Option<String>,
    entries: Mutex<Vec<Entry>>,
    /// The bond the node started with, `--co-bond` files included.
    started: MinerBond,
    /// What the node's templates sign with: `started` and the co-bonds here
    /// that are ready or registered.
    signing: Arc<RwLock<Option<MinerBond>>>,
    feed: Arc<RwLock<RelayFeed>>,
    info: Arc<RwLock<NodeInfo>>,
}

impl CoBondBook {
    /// The book at `path`, applying what it holds to `signing` at once.
    pub(crate) fn open(
        path: PathBuf,
        midstate_rpc: Option<String>,
        started: MinerBond,
        signing: Arc<RwLock<Option<MinerBond>>>,
        feed: Arc<RwLock<RelayFeed>>,
        info: Arc<RwLock<NodeInfo>>,
    ) -> Self {
        let entries = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                // Never overwrite what could not be read: keep it aside.
                let aside = path.with_extension("json.unreadable");
                tracing::warn!("{}: {e}; kept as {}", path.display(), aside.display());
                let _ = std::fs::rename(&path, &aside);
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        let book = Self {
            path,
            midstate_rpc,
            entries: Mutex::new(entries),
            started,
            signing,
            feed,
            info,
        };
        book.apply();
        book
    }

    /// This node's mining key (hex).
    pub fn mining_key(&self) -> String {
        hex::encode(self.started.mining_key())
    }

    /// This node's own bond's id (hex).
    pub fn bond_id(&self) -> String {
        hex::encode(self.started.bond_id)
    }

    /// Every co-bond in the book, newest first, and those `--co-bond` loaded.
    pub fn list(&self) -> Vec<CoBondView> {
        let entries = self.entries.lock().expect("co-bond book lock poisoned");
        let mut out: Vec<CoBondView> = entries.iter().rev().map(Entry::view).collect();
        let state = self.info.read().expect("node info lock poisoned").state.clone();
        for c in &self.started.co_bonds {
            if entries.iter().any(|e| e.coin.coin_id() == c.bond_id) {
                continue;
            }
            let on_chain = state.bonds.get(&c.bond_id);
            let phase = match on_chain {
                Some(e) if e.eligible_at(current_timestamp()) => "registered",
                Some(_) => "expired",
                None => "ready",
            };
            out.push(CoBondView {
                bond_id: hex::encode(c.bond_id),
                value: on_chain.map_or(0, |e| e.value),
                bonded_until: on_chain.map_or(0, |e| e.bonded_until),
                phase: phase.into(),
                progress: 100,
                note: "Loaded from a co-bond file at startup.".into(),
                added: 0,
            });
        }
        out
    }

    /// Takes `coin` as a co-bond. Blocking: it asks the midstate node for the
    /// coin. Returns its view, and whether the book took it just now (false
    /// when this node already knew it).
    ///
    /// # Formal Specification
    ///
    /// ```text
    /// Pre:  true
    /// Post: Ok((v, true))  ⇒ coin names this node's mining key, is on
    ///                        midstate, holds at least what a new bond must,
    ///                        counts now and is within the horizon, and the
    ///                        book holds it (saved)
    ///       Ok((v, false)) ⇒ this node already knew the bond; nothing changed
    ///                        except that a bond the chain registered on this
    ///                        key is now in the book
    ///       Err(_)         ⇒ the book is unchanged
    /// ```
    pub fn add(&self, coin: BondCoin) -> Result<(CoBondView, bool)> {
        let id = coin.coin_id();
        ensure!(
            coin.script.mining_key == self.started.mining_key(),
            "this bond names another mining key, so this node cannot sign with it"
        );
        ensure!(id != self.started.bond_id, "that is this node's own bond");
        if let Some(e) = self.find(&id) {
            return Ok((e.view(), false));
        }
        let now = current_timestamp();
        let state = self.info.read().expect("node info lock poisoned").state.clone();
        if self.started.co_bonds.iter().any(|c| c.bond_id == id) {
            let view = self.list().into_iter().find(|v| v.bond_id == hex::encode(id));
            return view.map(|v| (v, false)).ok_or_else(|| anyhow!("listing a startup co-bond"));
        }
        let (value, until) = (coin.value, coin.script.bonded_until);
        if let Some(entry) = state.bonds.get(&id) {
            // Registered already: the chain checked it, so sign with it.
            ensure!(entry.eligible_at(now), "this bond no longer counts");
            let e = Entry {
                coin,
                file: None,
                added: now,
                phase: "registered".into(),
                progress: 100,
                note: "Registered: this node signs with it.".into(),
                due: now,
            };
            return self.insert(e).map(|v| (v, false));
        }
        let asked = registration_min_bond(&state.target, now);
        ensure!(
            value >= asked,
            "a new bond must hold {} base units now; this one holds {value}",
            asked
        );
        ensure!(
            BOND_RULES.eligible(value, until, now),
            "its lock is too short: a bond counts only while more than {} days of its lock remain",
            min_remaining_bond_lock(now) / 1440
        );
        let horizon = est_midstate_height(now) + MAX_BOND_HORIZON;
        ensure!(until <= horizon, "its lock runs past midstate block {horizon}, further than midwimble registers");
        {
            let entries = self.entries.lock().expect("co-bond book lock poisoned");
            ensure!(entries.len() < MAX_CO_BONDS, "this node holds as many co-bonds as it takes");
        }

        // The coin must be on midstate: a proof of it is the first step of
        // its registration, and keeping it saves the first fetch.
        let rpc = self
            .midstate_rpc
            .as_deref()
            .ok_or_else(|| anyhow!("this node reads no midstate node (--midstate-rpc), so it cannot register co-bonds"))?;
        let v = RpcClient::new(rpc)
            .get(&format!("/utxo_proof/{}", hex::encode(id)))
            .map_err(|_| anyhow!("midstate has no such coin yet: hand it in once its lock confirms"))?;
        let proof = bond_proof_from_json(coin, &v)?;
        proof.verify(&bytes32(&v["state_root"])?)?;
        let mut file = BondFile::co_bond(coin.script.mining_key);
        file.coin = Some(coin);
        file.proof = Some(proof);
        let e = Entry {
            coin,
            file: Some(file),
            added: now,
            phase: "waiting".into(),
            progress: 0,
            note: "Handed in: this node registers it, which takes about a day.".into(),
            due: now,
        };
        self.insert(e).map(|v| (v, true))
    }

    fn find(&self, id: &[u8; 32]) -> Option<Entry> {
        let entries = self.entries.lock().expect("co-bond book lock poisoned");
        entries.iter().find(|e| e.coin.coin_id() == *id).cloned()
    }

    fn insert(&self, e: Entry) -> Result<CoBondView> {
        let view = e.view();
        {
            let mut entries = self.entries.lock().expect("co-bond book lock poisoned");
            if entries.iter().any(|x| x.coin.coin_id() == e.coin.coin_id()) {
                return Ok(view);
            }
            entries.push(e);
            self.save(&entries)?;
        }
        self.apply();
        Ok(view)
    }

    fn save(&self, entries: &[Entry]) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(entries)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("writing {}", self.path.display()))?;
        Ok(())
    }

    /// Puts what the book holds into what the node signs with: the bond it
    /// started with, and every co-bond here that is ready or registered.
    fn apply(&self) {
        let extra: Vec<CoBond> = {
            let entries = self.entries.lock().expect("co-bond book lock poisoned");
            entries.iter().filter_map(Entry::signer).collect()
        };
        {
            let mut feed = self.feed.write().expect("relay feed lock poisoned");
            for headers in extra.iter().filter_map(|c| c.midstate.as_ref()) {
                feed.add(headers, false);
            }
        }
        let mut bond = self.started.clone();
        bond.co_bonds.retain(|c| !extra.iter().any(|e| e.bond_id == c.bond_id));
        bond.co_bonds.extend(extra);
        *self.signing.write().expect("mining bond lock poisoned") = Some(bond);
    }

    /// One look at the book: marks what the chain registered or retired,
    /// takes at most one registration step, and applies the result.
    /// Blocking, since a step reads the midstate node.
    pub fn tick(&self) {
        let now = current_timestamp();
        let state = self.info.read().expect("node info lock poisoned").state.clone();
        let mut entries = self.entries.lock().expect("co-bond book lock poisoned").clone();
        let mut changed = false;
        for e in entries.iter_mut() {
            let id = e.coin.coin_id();
            let (value, until) = (e.coin.value, e.coin.script.bonded_until);
            let next = if let Some(entry) = state.bonds.get(&id) {
                if entry.eligible_at(now) {
                    Some(("registered", "Registered: this node signs with it."))
                } else {
                    Some(("expired", "Its lock has run down too far to count."))
                }
            } else if !BOND_RULES.eligible(value, until, now) {
                Some(("expired", "Its lock has run down too far to count."))
            } else if e.phase == "registered" {
                // The chain dropped it (a heavier midstate branch replaced
                // its proof's header): register it again.
                Some(("waiting", "Registering again: the chain no longer holds its registration."))
            } else {
                None
            };
            if let Some((phase, note)) = next {
                if e.phase != phase {
                    e.phase = phase.into();
                    e.note = note.into();
                    e.progress = if phase == "registered" { 100 } else { 0 };
                    e.due = if phase == "expired" { now + KEEP_EXPIRED_SECS } else { now };
                    changed = true;
                }
            }
        }
        let gone: Vec<[u8; 32]> = entries
            .iter()
            .filter(|e| e.phase == "expired" && now >= e.due)
            .map(|e| e.coin.coin_id())
            .collect();
        entries.retain(|e| !gone.contains(&e.coin.coin_id()));
        changed |= !gone.is_empty();

        // One step, for the co-bond waiting longest.
        let due = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.phase.as_str(), "waiting" | "burying" | "ready" | "failed") && now >= e.due)
            .min_by_key(|(_, e)| e.due)
            .map(|(i, _)| i);
        if let (Some(i), Some(rpc)) = (due, self.midstate_rpc.as_deref()) {
            let e = &mut entries[i];
            let midstate = RpcClient::new(rpc);
            match register_step(e.coin, e.file.clone(), &midstate, &state.target, state.relay.tip().height) {
                Ok(step) => {
                    e.file = Some(step.file);
                    e.phase = step.phase.into();
                    e.progress = step.progress;
                    e.note = step.note;
                }
                Err(err) => {
                    e.phase = "failed".into();
                    e.note = format!("The last step failed: {err:#}. It is tried again shortly.");
                }
            }
            e.due = now + retry_after(&e.phase);
            changed = true;
        }

        if changed {
            let mut held = self.entries.lock().expect("co-bond book lock poisoned");
            // Keep anything handed in while this step ran.
            for e in held.iter() {
                let id = e.coin.coin_id();
                if !gone.contains(&id) && !entries.iter().any(|x| x.coin.coin_id() == id) {
                    entries.push(e.clone());
                }
            }
            if let Err(err) = self.save(&entries) {
                tracing::warn!("saving co-bonds: {err:#}");
            }
            *held = entries;
            drop(held);
            self.apply();
        }
    }
}

/// Seconds until a co-bond in `phase` is looked at again.
fn retry_after(phase: &str) -> u64 {
    match phase {
        "waiting" | "failed" => 5 * 60,
        "burying" => 10 * 60,
        _ => 30 * 60,
    }
}

/// Where one registration step left a co-bond.
struct Step {
    file: BondFile,
    phase: &'static str,
    progress: u32,
    note: String,
}

/// One step of `midwimble bond register` for `coin`, carrying on from `held`:
/// a proof of the coin against midstate's tip (the one held, while the relay
/// can still carry it), and midstate's headers up through a day of
/// midwimble's work above it. Ready once that work is there.
fn register_step(
    coin: BondCoin,
    held: Option<BondFile>,
    midstate: &RpcClient,
    target: &[u8; 32],
    relay_tip: u64,
) -> Result<Step> {
    let mut f = held.unwrap_or_else(|| BondFile::co_bond(coin.script.mining_key));
    let anchor = crate::core::types::MIDSTATE_BLOCK_HEIGHT;
    // Midstate's `/state` height counts blocks, so its tip is one below.
    let midstate_tip = midstate.height().context("reading midstate's height")?.saturating_sub(1);
    let tip = midstate_tip.max(relay_tip);

    let keep = f
        .proof
        .clone()
        .filter(|p| p.coin == coin)
        .filter(|p| p.midstate_height > anchor && !HeldProof::at(p.midstate_height, tip).needs_refresh());
    let proof = match keep {
        Some(p) => p,
        None => {
            let v = midstate
                .get(&format!("/utxo_proof/{}", hex::encode(coin.coin_id())))
                .context("asking midstate for a proof of the bond coin")?;
            let p = bond_proof_from_json(coin, &v)?;
            p.verify(&bytes32(&v["state_root"])?)?;
            f.coin = Some(coin);
            f.proof = Some(p.clone());
            f.registration = None;
            f.midstate_headers = None;
            p
        }
    };
    ensure!(
        proof.midstate_height > anchor,
        "midstate's node is behind midwimble's anchor (block {anchor})"
    );

    let start = (relay_tip + 1).min(proof.midstate_height);
    let required = registration_work(target);
    let goal = required + required / 4;
    let cap = registration_cap(target);
    let mut run: Option<MidstateHeaders> = None;
    let mut credited = 0u128;
    'fetch: loop {
        let next = run.as_ref().map_or(start, |r| r.start + r.headers.len() as u64);
        let v = midstate.get(&format!("/headers/{next}/2000"))?;
        let headers = v["headers"].as_array().cloned().unwrap_or_default();
        if headers.is_empty() {
            break;
        }
        for h in &headers {
            let (link, prev) = header_link_from_json(h)?;
            let r = run.get_or_insert_with(|| MidstateHeaders { start, prev, headers: Vec::new() });
            ensure!(
                r.headers.last().map_or(prev == r.prev, |l| l.final_hash == prev),
                "midstate's headers do not link at height {}",
                r.start + r.headers.len() as u64
            );
            let height = r.start + r.headers.len() as u64;
            if height > proof.midstate_height {
                credited = credited.saturating_add(calculate_work(&link.target).min(cap));
            }
            r.headers.push(link);
            if credited >= goal || r.headers.len() >= MAX_RELAY_HEADERS_PER_BLOCK {
                break 'fetch;
            }
        }
    }
    let run = run.ok_or_else(|| anyhow!("midstate returned no headers from block {start}"))?;
    let registration = BondRegistration { proof };
    registration.check_carried(&run, target)?;
    let height = registration.proof.midstate_height;
    let burial = burial_headers(target, calculate_work(&run.headers[(height - run.start) as usize].target));
    f.registration = Some(registration);
    f.midstate_headers = Some(run);

    if credited >= required {
        let hours = window_left(height, tip).unwrap_or(0) / 60;
        return Ok(Step {
            file: f,
            phase: "ready",
            progress: 100,
            note: format!(
                "Ready: this node's next block registers it, if one is found within about {hours} hours. \
                 After that it takes a fresh proof and keeps going."
            ),
        });
    }
    let progress = (credited * 100 / required.max(1)).min(99) as u32;
    let note = if burial >= RELAY_WINDOW {
        format!(
            "At midwimble's difficulty a proof needs about {burial} midstate blocks above it and the relay \
             keeps only {RELAY_WINDOW}: no bond can register until the difficulty falls."
        )
    } else {
        format!("Waiting for a day of midwimble's work above its proof: {progress}% there.")
    };
    Ok(Step { file: f, phase: "burying", progress, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coin(key: [u8; 32], until: u64, value: u64) -> BondCoin {
        BondCoin {
            script: crate::core::bond::BondScript { mining_key: key, bonded_until: until, owner_pk: [7; 32] },
            value,
            salt: [9; 32],
        }
    }

    fn entry(phase: &str) -> Entry {
        Entry {
            coin: coin([1; 32], 10_000_000, 1 << 37),
            file: None,
            added: 1,
            phase: phase.into(),
            progress: 0,
            note: String::new(),
            due: 0,
        }
    }

    #[test]
    fn only_ready_and_registered_co_bonds_sign() {
        assert!(entry("registered").signer().is_some_and(|c| c.registration.is_none()));
        assert!(entry("ready").signer().is_none(), "ready needs its registration in hand");
        let mut ready = entry("ready");
        ready.file = Some(BondFile::default());
        assert!(ready.signer().is_some());
        for phase in ["waiting", "burying", "failed", "expired"] {
            assert!(entry(phase).signer().is_none(), "{phase}");
        }
    }

    #[test]
    fn a_book_survives_a_restart() {
        let e = entry("burying");
        let bytes = serde_json::to_vec(&vec![e.clone()]).unwrap();
        let back: Vec<Entry> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back[0].view(), e.view());
        assert_eq!(back[0].coin, e.coin);
    }

    #[test]
    fn steps_are_spaced_by_phase() {
        assert!(retry_after("waiting") < retry_after("burying"));
        assert!(retry_after("burying") < retry_after("ready"));
        assert_eq!(retry_after("failed"), retry_after("waiting"));
    }
}
