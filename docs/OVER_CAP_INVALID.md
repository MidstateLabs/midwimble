# Over-cap blocks invalid (from height 50,000)

> From height 50,000 (`bond::OVER_CAP_INVALID_FROM`), a signed block whose
> bond has signed more than its cap of the last 1,440 blocks is invalid.
> Below that height nothing changes.

This is a hard fork. Every node, pool and merge miner must run a release
that carries it before the chain reaches height 50,000. A node on an older
release keeps accepting over-cap blocks, and leaves the network the first
time one is mined at or above that height.

## The rule

The per-bond cap (`docs/BONDED_POW.md`, "Co-bonding and the per-bond cap")
already has a test: a signed block is *over its cap* when its bond has
signed `n` of the last `CAP_WINDOW` = 1,440 blocks, this block included, and

```text
n > signer_cap = ceil(1440 / clamp(eligible bonds, 1, CAP_K))      CAP_K = 360
```

with eligibility judged at the block's own timestamp and the bond set as the
block leaves it, its own registration included. The same test has, until
now, done two things:

- **weight:** the block adds `1 / OVER_CAP_WORK_DIVISOR` = 1/1024 of its work
  to fork choice, a *sliver*, instead of all of it (`bond::credit_in_window`);
- **pay:** from the mainnet switch it mints `reward × cap / n` of its subsidy,
  and the rest is burned (`bond::capped_subsidy`).

From `OVER_CAP_INVALID_FROM` it does a third, which replaces both:

| Height of the block | Signed, `n <= signer_cap` | Signed, `n > signer_cap` | Unsigned |
|---|---|---|---|
| below 50,000 | valid, full work, full subsidy | valid, sliver of work, tapered subsidy | unchanged rule (`BONDED_MINING_FROM`) |
| 50,000 and above | valid, full work, full subsidy | **invalid** | unchanged rule (`BONDED_MINING_FROM`) |

Precisely:

- **The same test, with no allowance.** `n == signer_cap` is valid and counts
  in full; `n == signer_cap + 1` is invalid. There is no buffer for luck: a
  producer that wins more blocks than its bonds cover has to wait, or bond
  more.
- **Keyed on height.** The block's own height decides which rule applies:
  the block at 49,999 is judged by the old rule, the block at 50,000 by the
  new one. Everything the test reads (`n`, the cap, eligibility) is computed
  exactly as before.
- **Where it is checked.** `state::apply_batch`, step 5a-i, calls
  `bond::check_signer_cap` right after the signer window has counted the
  block, before the block's claimed chain weight is compared. An over-cap
  block is rejected with

  ```text
  block 50012 is over its bond's cap: bond <id> has signed 5 of the last 1440
  blocks, this one included, against a cap of 4; from height 50000 an
  over-cap block is invalid
  ```

- **Headers.** Headers-first sync checks each header's claimed chain weight
  against what its block could add (`bond::credit_possible`). From the height
  a signed block can only add its full work (as far as the work cap allows),
  so a signed header claiming the sliver is refused before any block is
  downloaded. Below the height both remain possible, as before.
- **Templates.** A producer refuses to build an over-cap block from the
  height (`template::build_template_*` calls the same `check_signer_cap`),
  rather than spend work on a block that every upgraded node rejects. It
  signs with whichever of its bonds has signed fewest of the window, so when
  that one is over its cap, all of them are, and the node logs:

  ```text
  Cannot build a block template: block 50012 is over its bond's cap: ...
  Every bond this producer can sign with is at its cap: it can mine again
  once other producers' blocks have moved the window on, or with more bonds
  registered
  ```

  The template check is a courtesy to honest miners. The consensus check is
  the rule: a modified miner can skip the first, never the second.

## Why

An over-cap block is valid but weighs a sliver, so one block that counts in
full outweighs up to 1,023 of them. That is what makes orphaning over-cap
runs work (`core/orphan.rs`). It is also an attack.

**The amplified selfish-mining vector.** Suppose the tip is a run of over-cap
blocks, all slivers. A miner under its cap finds a full-weight block on the
last full-weight parent, below the run. That one block already outweighs the
whole run. Published at once, every other producer under its cap starts
building on it. Withheld, only its finder builds there: it mines more
full-weight blocks on top in private, as long as its bonds' quota lasts,
while the public network keeps extending the sliver run, which adds almost
nothing to the public chain's weight. When it chooses, it releases a chain
that is shorter but heavier, and every node follows it. The run, and
everything mined on it while the private chain grew, is replaced.

In ordinary selfish mining on a heaviest-chain coin, the withholding miner
races all the honest hashrate. Here it races only the producers under their
cap building on the same parent. The hashrate extending the sliver run is
spent on blocks that weigh almost nothing, and so is no competition at all.
The longer the run, the cheaper the attack.

**Why local limits do not close it.** Three numbers bound how far back a
node will go, and none of them is consensus:

- `orphan::MAX_ORPHAN_DEPTH` (100): how far below the tip this node builds,
  and how far back it takes a heavier block straight off the network without
  a sync session;
- `sync::MAX_REORG_DEPTH` (1,000): how far back a sync will reorganise;
- the finalized checkpoint (`finality.rs`): a node never reorganises below
  it.

All three are policy of this node. Fork choice is, and always was, the
heaviest valid chain. A modified miner can build on any parent it likes: it
ignores `--max-orphan-depth`, `--no-orphan-over-cap` and every other
templating limit. An unmodified node offered its heavier chain through a
normal sync still takes it, up to the node's own acceptance limits. Shorter
orphan depths or tighter finality only narrow the window; they cannot close
it, and they narrow honest replacements of over-cap runs just as much.

**Why invalidity does.** The attack needs a run of slivers on the public
chain to hide behind. If an over-cap block is invalid, that run can never be
on the chain: every valid block from the height counts in full, so the public
chain grows by full-weight blocks as fast as its producers' bonds allow, and a
withholding miner races all of them. That is the only rule change that removes
this family of attacks rather than shrinking it.

What remains is classic selfish mining on a full-weight chain, as on any
heaviest-chain coin. The per-bond cap and the work cap still bound it: a
private chain is under the same cap as the public one, so outweighing the
honest chain still needs more full-weight blocks, and so more bonds, than it
has.

## Consensus and policy

| | Kind | Changed here |
|---|---|---|
| Fork choice: the heaviest valid chain, work less what the caps withhold | consensus | no |
| The per-bond cap's formula, `CAP_WINDOW`, `CAP_K`, `OVER_CAP_WORK_DIVISOR` | consensus | no |
| Unsigned blocks (`BONDED_MINING_FROM`) | consensus | no |
| The taper and the mainnet switch (`EARLY_BONDS_UNTIL`) | consensus | no, below the height; from it no valid block is over its cap, so the taper withholds nothing |
| **An over-cap block is invalid from `OVER_CAP_INVALID_FROM`** | **consensus** | **yes: the only consensus change** |
| Orphaning over-cap runs, `--max-orphan-depth`, `--no-orphan-over-cap` | node policy | no |
| `MAX_REORG_DEPTH`, finality | node policy | no |

Orphaning only decides where this node builds its own blocks. It never made
any block valid or invalid, and it does not now.

## Activation

The rule activates at **height 50,000** on mainnet. On 2026-11-12 UTC the
chain was at about height 41,800, so at 60 s blocks the height is about five
and a half days later: around 2026-11-18 UTC. The exact time depends on how
fast blocks come in; the height is what counts.

The release must be public, with the height, well before then, for two
reasons:

- **Bonds.** From the height a producer can mine at most its bonds' caps.
  With 360 or more eligible bonds that is four blocks a bond a day, so a
  producer with a share `s` of the hashrate needs about `360 × s` bonds to
  use all of it (108 at 30%). A bond takes a day of midstate work above its
  coin to register, so producers need the notice to lock and register them
  before the height.
- **Upgrades.** A hard fork is decided by the hashrate that follows it. If
  only a minority of nodes enforce the rule on the night, they are the
  minority fork: unupgraded hashrate keeps mining over-cap blocks on the old
  chain, and the upgraded nodes reject that chain from the first over-cap
  block at or above the height.

Test and devnet builds (`fast-mining`) activate at height 128, eight signer
windows, so the tests cross the boundary on a short chain.

A build refuses to compile with a mainnet height at or below
`OVER_CAP_INVALID_NOT_BEFORE` (two days of blocks past the 2026-11-12 tip), so
the height cannot be set, by mistake, to one the chain has already passed.
Moving the height after a release is itself a consensus change.

## A producer already over its cap

A producer whose bonds have signed more than their caps of the last 1,440
blocks when the height arrives cannot produce a valid block until either:

- enough of its blocks have left the window, which happens only as *other*
  producers' blocks are added (the window is the last 1,440 blocks, not the
  last day); or
- it registers more bonds. A new bond has signed nothing in the window, and
  may sign the block that registers it.

The cap is the same for every bond, whatever it locks: a 1,000 gMDS bond
covers exactly as many blocks as the minimum.

## Liveness

Before the height, the chain could never stall: a lone producer could always
extend it, over its cap if need be, because an over-cap block was valid. From
the height that is no longer true. A valid block needs a bond under its cap,
and a block may be unsigned only while no registered bond could sign it.

So the chain keeps moving only while the bonds that are actually mining have
quota for the window between them:

```text
(bonds mining) × signer_cap  >=  CAP_WINDOW (1,440)
```

`signer_cap` is set by the *eligible* bonds, mining or not. Every eligible
bond that nobody mines with lowers the others' cap without adding quota of
its own: with 360 or more eligible, each covers four blocks a window, so at
least 360 of them must be signing. If the bonds in use have all signed their
caps, no valid block can be built and the window does not move, because it
moves only with blocks. The chain resumes when a new bond registers (in a
block it signs itself), or when idle bonds stop being eligible as their locks
run down.

Before the height, operators should check, on an upgraded node, that the
bonds being mined with have enough quota: `GET /bonds` lists the eligible
bonds (`eligible_now`), and the explorer's signer window shows how many of the
last 1,440 blocks each has signed against the current cap.

## History, the taper and the burn tracker

Nothing below the height changes. Every over-cap block already on the chain
stays valid, keeps the sliver of weight it was given and the tapered subsidy
it minted, and a node replaying or syncing that history accepts it exactly
as before. The burn tracker (`/#/burn`) keeps counting what the taper
withheld below the height.

From the height, no valid block is over its cap, so the taper withholds
nothing and no block weighs a sliver for being over its cap. The taper code
stays: it still has the old history to judge, and it is a no-op on new
blocks. Unsigned blocks still weigh a sliver from `BONDED_MINING_FROM`, as
before.

## Orphaning

Orphaning over-cap runs (`core/orphan.rs`) stays as it is. Below the height
it still replaces runs of other producers' over-cap blocks, with the same
defaults and flags. From the height an upgraded node never sees a valid
over-cap block, so there is never a run to replace and orphaning simply finds
nothing to do among upgraded peers. It is left in place rather than removed:
it still serves the history below the height, and removing it is a separate
change.

## Upgrading

1. Run a release that carries `OVER_CAP_INVALID_FROM` = 50,000 before the
   chain reaches height 50,000. `midwimble params` prints the height
   (`over-cap blocks`), and `GET /bonds` reports `over_cap_invalid_from` and
   whether the next block is past it (`over_cap_invalid`).
2. Producers: register enough bonds for your hashrate before the height,
   about `360 × s` for a share `s`, and check that the bonds you sign with
   cover what you mine (see "Liveness").
3. Pools and merge miners: upgrade the node your templates come from. A
   template from an old node can be over its cap, and would be rejected.
4. Nothing to configure. The height is compiled in; there is no flag to
   change it.

## Tests

- `core::bond::cap_tests::over_cap_blocks_are_invalid_from_the_activation_height`:
  the rule on its own, at a chosen height: below it an over-cap block passes;
  from it `n == cap` passes and `n == cap + 1` fails; unsigned blocks always
  pass.
- `core::bond::cap_tests::the_builds_activation_height_is_the_boundary`: the
  build's own height, `H - 1` against `H`, and headers-first sync agreeing.
- `core::chain_tests::over_cap_blocks_are_invalid_from_the_activation_height`
  (`fast-mining`): a real chain through `apply_batch` across height 128. An
  over-cap run below the height is valid, weighs a sliver and is tapered, up
  to `H - 1`; at `H` the template refuses and a block built regardless is
  rejected; from `H` a block at the cap counts in full and the next is
  rejected; the chain carries on with a bond under its cap.
- The existing taper, sliver, storage and orphaning tests run on chains
  below the height, and pass unchanged.
