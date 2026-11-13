# Over-cap blocks invalid (from height 50,000)

> From height 50,000 (`bond::OVER_CAP_INVALID_FROM`), a signed block whose
> bond has signed more than its cap of its window is invalid. The window is
> the last 1,440 blocks, less any stamped a day or more before the block
> being judged, and the cap is an equal share of it among the bonds in use:
> the eligible bonds that signed at least one other block of the window.
> Below that height nothing changes.

This is a hard fork. Every node, pool and merge miner must run v1.4.1-DIMEBAG
or later before the chain reaches height 50,000. v1.4.0-DIMEBAG carried the
same height with neither the day limit nor the share among bonds in use (see
"Why the window also ages by time" and "Why the cap is shared among the bonds
in use"); it, and every older release, rejects blocks this rule accepts, and
leaves the network the first time one is mined at or above the height.

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

| Height of the block | Signed, `n <= cap` | Signed, `n > cap` | Unsigned |
|---|---|---|---|
| below 50,000 | valid, full work, full subsidy | valid, sliver of work, tapered subsidy | unchanged rule (`BONDED_MINING_FROM`) |
| 50,000 and above | valid, full work, full subsidy | **invalid** | unchanged rule (`BONDED_MINING_FROM`) |

From the same height, two parts of the test change. Both only loosen it, and
together they keep the rule from stopping the chain.

**The window ages by time.** `n` counts the bond's blocks among the last
`CAP_WINDOW` = 1,440, this one included, leaving out any stamped
`CAP_WINDOW_SECS` = 86,400 seconds (a day) or more before this one
(`bond::signed_in_window`):

```text
n(c) = #{ b in c and its 1,439 predecessors :
          signer(b) = signer(c)  and  timestamp(b) > timestamp(c) - 86,400 }
```

**The cap is shared among the bonds in use.** A bond is in use for block `c`
when it is eligible (at `c`'s timestamp, in the set as `c` leaves it) and
signed at least one of the 1,439 blocks before `c` (`bond::bonds_in_use`,
`bond::signer_cap_in`):

```text
in_use(c) = #{ bonds eligible at c that signed at least one of c's 1,439 predecessors }
cap(c)    = ceil(1440 / clamp(in_use(c), 1, CAP_K))
```

`c` itself never counts, so the cap is the same whoever signs it, and a
producer knows it before it builds the block (`bond::next_signer_cap`). A bond
comes into use with its first block and leaves it once its last block has
left the window. In-use is counted in blocks, not aged by time (see "A chain
mined in private"). The bonds in use are some of the eligible ones, so the
cap is never below `signer_cap`.

Whenever the last 1,440 blocks span less than a day, as they do while the
chain keeps to its one-a-minute schedule or runs ahead of it, the day limit
leaves nothing out and `n` is exactly the 1,440-block count. It binds only
when blocks come slower, which is when a window counted in blocks alone can
stop moving (see "Why the window also ages by time"). Below the height the
window is the last 1,440 blocks and the cap is `signer_cap`, as they always
were: the taper and the sliver do not age.

Precisely:

- **The same test, with no allowance.** `n == cap` is valid and counts in
  full; `n == cap + 1` is invalid. There is no buffer for luck: a producer
  that wins more blocks than its bonds cover has to wait, or bond more.
- **Keyed on height.** The block's own height decides which rule applies:
  the block at 49,999 is judged by the old rule, the block at 50,000 by the
  new one, the aging and the bonds in use included.
- **The same `n` and cap for validity, weight and pay.** Fork choice's sliver
  (`bond::credit_in_window`) and the taper (`bond::subsidy_for`) read the
  same `n` against the same cap, so from the height a valid signed block
  counts in full and mints the whole reward, even when it is over its cap
  counted in blocks alone or against every eligible bond.
- **Where it is checked.** `state::apply_batch`, step 5a-i, calls
  `bond::check_signer_cap` right after the signer window has counted the
  block, before the block's claimed chain weight is compared. An over-cap
  block is rejected with

  ```text
  block 50012 is over its bond's cap: bond <id> has signed 41 of its window
  (the last 1440 blocks, less any stamped 86400s or more before this one),
  this one included, against a cap of 40, its share among the 36 eligible
  bonds that signed the window's other blocks; from height 50000 an over-cap
  block is invalid
  ```

- **Where the window comes from.** Signers and timestamps are both in every
  header, so storage (`storage::rebuild_fork_choice_windows`, and the replay
  in `commit_chain`) and snapshots rebuild the window, and with it which
  bonds are in use, from what they already keep; nothing new is stored, and
  nothing outside the headers is trusted.
- **Headers.** Headers-first sync checks each header's claimed chain weight
  against what its block could add (`bond::credit_possible`). From the height
  a signed block can only add its full work (as far as the work cap allows),
  so a signed header claiming the sliver is refused before any block is
  downloaded. Below the height both remain possible, as before. The cap
  itself is checked when the block is applied, as before.
- **Templates.** A producer refuses to build an over-cap block from the
  height (`template::build_template_*` calls the same `check_signer_cap`),
  rather than spend work on a block that every upgraded node rejects. It
  signs with whichever of its bonds has used least of the window the block
  would be judged by (`bond::quota_used`, at the template's timestamp), and
  the cap is the same whichever signs, so when that one is over its cap, all
  of them are, and the node logs:

  ```text
  Cannot build a block template: block 50012 is over its bond's cap: ...
  Every bond this producer can sign with is at its cap: it can mine again
  once its oldest signatures in the window are 86400 seconds old or other
  producers' blocks have moved the window past them, or with more bonds
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
private chain is held to the same cap as the public one (see "A chain mined
in private"), so outweighing the honest chain still needs more full-weight
blocks, and so more bonds, than it has.

## Why the window also ages by time

v1.4.0-DIMEBAG made the 1,440-block test a validity rule on its own. That
can stop the chain, and on mainnet as it stood it would have.

**The stall.** `signer_cap` divides the window by every *eligible* bond,
mining or not, so the bonds actually mining can sign at most
`A x signer_cap` of any 1,440 blocks, `A` being how many are mining. Whenever
eligible bonds that nobody mines with keep that under 1,440, the bonds in use
reach their caps, no valid block can be built (a block may be unsigned only
while no bond could sign it), and a window counted in blocks never moves
again. New honest bonds barely help: each one also lowers the cap. It ends
only when the idle bonds lose eligibility or start mining, or when enough new
bonds register, one a block.

At height 42,881 (2026-11-13) 36 bonds were eligible, for a cap of 40
(36 x 40 = 1,440 exactly). Four of them, on one mining key (`12daf354...`),
had not signed since about block 38,900, and stay eligible until about
31 December 2026. The 32 in use can sign 1,280 blocks of 1,440, and they had
already signed about 45 each of the window on average. Simulated, v1.4.0
makes a few dozen blocks past 50,000 and then stops until the four lapse, six
weeks later; ASERT, six weeks behind schedule by then, would then let tens of
thousands of almost free blocks through. Anyone could cause the same on
purpose by registering bonds and not mining with them.

**The fix.** A signature leaves the window a day after its block, however few
blocks follow it. A stall can then last at most a day: every bond that is
mining can sign again within a day, whatever the other bonds do. The next
block's own timestamp is what shows the day has passed.

**Why keep the block count as well.** A window measured in time alone caps
the chain at `(bonds sharing it) x cap` blocks a day, which is exactly 1,440
whenever their number divides 1,440 (36 now, 32 once the four lapse). The
chain could then never run ahead of schedule, so ASERT could never win back
the time a slow period cost it: simulated on today's numbers, difficulty fell
by a factor of about 2^29 while the four idle bonds were eligible and stayed
there after they lapsed. Keeping the 1,440-block count, and aging within it,
leaves the rule exactly as it was while blocks come on schedule or faster, so
the chain can run ahead to make up lost time.

**Why block timestamps.** Every header carries its block's timestamp, under
its proof of work, so storage, snapshots and headers-first sync have the
window's clock without storing or trusting anything new. Midstate's relay
height would serve as a clock too, but no header carries the relay height a
block left behind: storage's undo records reach back 1,200 blocks, pruned
nodes delete block bodies, a snapshot could not prove it, and a window aged
by relay height would stop again whenever nobody relays. The timestamps are
bounded where it matters:

- a node accepts a block stamped at most 15 minutes past its own clock
  (`types::MAX_FUTURE_BLOCK_TIME`), and a block must be stamped past the
  median of the eleven before it (about six blocks back), so by choosing its
  timestamps a producer shortens its own window by twenty minutes or so at
  most while blocks come about once a minute, about 1.5% more quota;
- a chain mined in private can be stamped no later than the moment it is
  published, so it gets no more quota than the time it took, as if it had
  aged by any other clock;
- a block stamped far ahead makes the next block's target easier by ASERT,
  and fork choice then credits every block for a week at most a quarter above
  that easy one (`bond::cap_work`), so a chain that games its timestamps
  cripples its own weight; and since a block's own timestamp ages only its
  own window, a single block stamped ahead frees nothing for the blocks after
  it.

Bond expiry already runs on these timestamps (`bond::est_midstate_height`).

## Why the cap is shared among the bonds in use

The aging alone keeps the chain moving, but no faster than the bonds in use
can sign: with idle bonds counted in `signer_cap`, 1,280 blocks a day on the
numbers above, with ASERT easing the difficulty by about 2.7 hours of
schedule a day, a halving every day and a half or so, for as long as the
idle bonds stay eligible. Sharing the cap among the bonds in use removes
that: idle bonds no longer lower anyone's cap, and the bonds that are mining
can always cover the window between them,

```text
in_use x ceil(1440 / in_use) >= 1,440        (in_use <= 360; each covers 4 past that)
```

so the chain keeps its pace however many registered bonds sit idle. On
today's numbers the 32 bonds in use get a cap of 45 (32 x 45 = 1,440).
Registering bonds and not mining with them can no longer slow the chain at
all.

Run through the consensus code at mainnet's parameters, with today's bonds
(32 in use, 4 idle), each of the 32 holding 45 blocks of the window, about
what they hold now, and a block offered every minute for four days:

| From height 50,000 | Blocks per day | Longest gap | Cap |
|---|---|---|---|
| v1.4.0: block count, every eligible bond | none: every bond is over its cap | four days and counting | 40 |
| the aging alone | 1,280 | 161 min, every day | 40 |
| the aging and the bonds in use (v1.4.1) | 1,440 | 1 min | 45 |

**Each needs the other.** The share among bonds in use does not replace the
aging. A bond that stops mining stays in use until its last block has left
the window, and a window counted in blocks only moves when blocks are added.
If the bonds still mining are at their caps by then, as they are whenever the
bonds in use cover the window exactly, no block can be added and the stopped
bond never leaves: the chain stops until the stopped bond mines again or
lapses. Simulated without the aging, 33 bonds with one stopping, 36 with four
and 40 with four all froze, the last two at once, and stayed frozen for the
five days simulated; with it, none lost more than a few minutes, and the cap
rose (to 45, 45 and 40) as the stopped bonds' blocks left the window.

**A chain mined in private.** Counting every eligible bond kept an attacker's
private chain, where only the attacker signs, under the same cap as the
honest one. Counting the bonds in use keeps it there for as long as it
matters, because in-use is counted in blocks, not by time: the bonds that
signed the blocks a private chain forks from stay in use on it, however its
own blocks are stamped, until it has pushed their last blocks out of the
window with a window's worth of its own. Until then the attacker is held to
the honest chain's cap or below, and the aging lets each of its bonds sign no
more than that cap a day, so with `k` bonds against `h` honest ones in use,
getting there takes it about `1 + h / k` days. Meanwhile the honest chain,
from which the attacker's bonds have gone quiet, drops them from use within
its own window and runs at 1,440 a day. Bonds held dormant on the honest
chain no longer slow it either, where under `signer_cap` they lowered its cap
until they woke on the private chain. In simulation, with the attacker's
bonds dormant on the honest chain, winning a day-long private race took
60-64% of the bonds under this rule, against 50% when every eligible bond
counted.

**What it does not fix.**

- *A bond that signs now and then.* A bond stays in use with one block in
  every 1,440, so an operator who wants to slow the chain can keep a bond in
  use while adding almost nothing: each such bond costs the chain about
  `cap - 1` blocks a day, as an idle one did before. Unlike idle bonds, this
  takes hashrate: a valid block per bond per day, at full difficulty, in
  the open, on the explorer. A share weighted by how much each bond signs
  would close it, but would also let a private chain shed honest bonds'
  weight from its first block; it is left for later.
- *A window with nothing to spare.* When the bonds in use divide 1,440
  exactly, their quota covers the window with nothing to spare, so even with
  every bond mining a block often has to wait for one particular bond, and
  difficulty settles lower than the hashrate alone would set it (about 2^5
  lower in simulation, with or without these changes).
- *Saved quota.* A bond that has signed nothing for a day can sign its whole
  cap at once, and a bond that comes back into use lowers the others' caps
  again. The work cap bounds what such a burst weighs.

## Consensus and policy

| | Kind | Changed here |
|---|---|---|
| Fork choice: the heaviest valid chain, work less what the caps withhold | consensus | no |
| `CAP_WINDOW`, `CAP_K`, `OVER_CAP_WORK_DIVISOR` | consensus | no |
| Unsigned blocks (`BONDED_MINING_FROM`) | consensus | no |
| The taper and the mainnet switch (`EARLY_BONDS_UNTIL`) | consensus | no, below the height; from it no valid block is over its cap, so the taper withholds nothing |
| **An over-cap block is invalid from `OVER_CAP_INVALID_FROM`** | **consensus** | **yes** |
| **From the same height the window ages: a signature leaves it `CAP_WINDOW_SECS` after its block** | **consensus** | **yes (v1.4.1)** |
| **From the same height the cap is a share among the bonds in use, not every eligible bond** | **consensus** | **yes (v1.4.1)** |
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
  With 360 or more bonds in use that is four blocks a bond a day, so a
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

A producer whose bonds have signed more than their caps of the window when
the height arrives cannot produce a valid block until one of:

- enough of its blocks have left the last 1,440, as other producers' blocks
  are added;
- enough of its blocks are a day older than the block it would sign (it can
  stamp that block no further ahead than nodes' clocks allow);
- bonds leave use as their last blocks leave the window, which raises the
  cap; or
- it registers more bonds. A new bond has signed nothing in the window, and
  may sign the block that registers it.

The cap is the same for every bond, whatever it locks: a 1,000 gMDS bond
covers exactly as many blocks as the minimum.

## Liveness

Before the height, the chain could never stall: a lone producer could always
extend it, over its cap if need be, because an over-cap block was valid. From
the height a valid block needs a bond under its cap, and a block may be
unsigned only while no registered bond could sign it. Two things keep that
from stopping or slowing the chain:

- **The aging.** Every signature leaves the window a day after its block, so
  any bond that is mining can sign again within a day, whatever the other
  bonds do. No stall lasts more than a day.
- **The share among bonds in use.** The bonds signing the window cover it
  between them, so idle bonds cost nothing:

  ```text
  (bonds in use) x cap  >=  CAP_WINDOW (1,440)
  ```

The chain runs short of 1,440 blocks a day only while bonds that are in use
stop or slow down: a bond that stops mining keeps its share of the window
until its last block leaves it, a day at most on schedule, and a bond that
signs only now and then keeps it indefinitely (see "What it does not fix").

Before the height, operators should check, on an upgraded node, that the
bonds being mined with have enough quota: `GET /bonds` lists the eligible
bonds (`eligible_now`) and reports the next block's cap (`signer_cap`), the
bonds in use (`bonds_in_use`) and `over_cap_window_secs`; the explorer's
signer window shows how much of the next block's window each bond has used
(`window_signed`, from `bond::quota_used`) against that cap (`cap`, with
`in_use` and `eligible`).

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

1. Run v1.4.1-DIMEBAG or later before the chain reaches height 50,000:
   every node, not only producers. v1.4.0-DIMEBAG is not enough, and nor is
   anything older: they reject blocks this rule accepts. `midwimble params`
   prints the height, the day limit and the share among bonds in use
   (`over-cap blocks`), and `GET /bonds` reports `over_cap_invalid_from`,
   whether the next block is past it (`over_cap_invalid`),
   `over_cap_window_secs`, `signer_cap` and `bonds_in_use`.
2. Producers: register enough bonds for your hashrate before the height,
   about `360 × s` for a share `s`, and check that the bonds you sign with
   cover what you mine (see "Liveness"). A bond you stop mining with keeps a
   share of the window until its blocks leave it; one you never mine with
   costs nobody anything.
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
- `core::bond::cap_tests::from_the_height_a_signature_leaves_the_window_a_day_after_its_block`:
  the aging on its own: nothing below the height, the exact second a
  signature leaves, blocks stamped after the one judged still counting, and
  the 1,440-block count still applying however close together blocks are
  stamped.
- `core::bond::cap_tests::over_its_cap_in_blocks_but_not_in_time_is_valid_from_the_height`:
  validity, weight and pay agree on the aged `n`; below the height the same
  block is a tapered sliver.
- `core::bond::cap_tests::quota_used_is_what_the_next_blocks_window_holds`:
  what producers choose their signer by is exactly `n - 1`, on a long,
  irregular chain.
- `core::bond::cap_tests::from_the_height_the_cap_is_shared_among_the_bonds_in_use`:
  the bonds in use on their own: idle bonds, bonds that can no longer sign,
  unsigned blocks and a bond's own first block count for nothing, whoever
  signs and however the blocks are stamped; a bond leaves use when its last
  block leaves the window; the cap never falls below `1 / CAP_K`; below the
  height every eligible bond counts.
- `core::bond::cap_tests::the_next_blocks_cap_is_the_same_whoever_signs_it`:
  what producers and the explorer work out before a block exists
  (`next_signer_cap`, `next_bonds_in_use`) is the cap consensus applies, for
  every signer, on a long, irregular chain, and never below `signer_cap`.
- `core::bond::cap_tests::on_a_private_chain_the_honest_bonds_stay_in_use_for_a_window`:
  a private chain whose blocks are stamped ten days apart keeps every honest
  bond in use, and its own cap at or below the honest one, until it has made
  a window of blocks of its own.
- `core::chain_tests::from_the_height_an_idle_bond_does_not_slow_the_chain`
  (`fast-mining`): mainnet's situation through `apply_batch`: three eligible
  bonds, two mining. Below the height they are over the cap; from it the
  idle bond is not in use, the cap is 8 of 16, and the two keep a block a
  minute, every block valid, in full and paid in full, though each is over
  the old cap. When the idle bond signs again the cap falls back.
- `core::chain_tests::a_bond_that_stops_mining_cannot_stall_the_chain`
  (`fast-mining`): three bonds mining, one stops at the height. The other two
  reach their caps with its blocks still in the window, stuck counted in
  blocks; a block stamped a second before enough of a bond's signatures are a
  day old is rejected, the one at that second is valid, counts in full and
  pays the whole reward; once the stopped bond's blocks have left the window
  the cap rises, and the two keep any pace between them.
- `storage::tests::storage_judges_the_signer_window_exactly_as_the_node_does`
  (`node`, `fast-mining`): the same story through storage: it replays blocks
  valid only by the aging, and blocks valid only because a bond left use, at
  the node's weight, and a restart or a rewind rebuilds the window,
  timestamps and bonds in use included.
- `core::chain_tests::over_cap_blocks_are_invalid_from_the_activation_height`
  (`fast-mining`): a real chain through `apply_batch` across height 128. An
  over-cap run below the height is valid, weighs a sliver and is tapered, up
  to `H - 1`; at `H` the template refuses and a block built regardless is
  rejected; from `H` a block at the cap counts in full and the next is
  rejected; the chain carries on with a bond under its cap.
- The existing taper, sliver, storage and orphaning tests run on chains
  below the height, and pass unchanged.
