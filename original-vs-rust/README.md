# Experiments on the original (Haskell) that fix the behaviour the Rust port copies

Each `.hs` file runs a series of scenarios with the Haskell implementation and `*.output-original.txt` keeps what it
gave. The tests in `transient-core/tests/` and `transient-loggable/tests/` reproduce these scenarios and expect that output.

These are reference captures, not automated tests: nothing runs them or compares their output. The expected values in the Rust
tests were copied from here by hand. They are the evidence of what the original does, and each one was run against the Haskell
library of the `transient-stack` repository.

## How to run them

These programs run against the Haskell library, so they are compiled from the `transient-stack` repository (the `transient`
package already built in `dist-newstyle`; `runghc` does not see it, you have to compile). From `transient-stack/`, with
`$RS` the path of this repository:

    cabal exec --offline -- ghc -package transient -threaded -outputdir /tmp/o -o /tmp/exp $RS/original-vs-rust/OnFinish.hs
    /tmp/exp

(add `-package transient-universe` for `LogFormat`, `SelfRunAt` and `WebPost`, which use `Transient.Move`).

The `keep'` calls are wrapped in `System.Timeout.timeout` (imported qualified: Transient exports its own `timeout`);
a scenario that says "(keep' does not finish)" means that listeners are still alive, not that the experiment failed.

## What each one checks

| File | Scenarios | Rust test |
|---|---|---|
| `OnFinish.hs` | a registration runs once; the `onFinish` statement runs once each time it is executed | `tests/evar_mailbox.rs` (`on_finish_*`) |
| `EVarsMailboxes.hs` | E1–E5 (EVars: LIFO readers, synchronous write, `readEVar1`, `delReadEVar`) and M1–M6 (mailboxes by type and key, `lastPutMailbox`, no queue) | `tests/evar_mailbox.rs` |
| `DeleteThisMailboxReader.hs` | M7: `deleteThisMailboxReader` leaves the other readers | `delete_this_mailbox_reader_*` |
| `Exceptions.hs` | X1–X15: `onException` (several threads, filter by type, LIFO, `continue`, `throwt` versus `throwIO`, `localExceptions`, `cutExceptions`...) | `tests/exceptions.rs` |
| `OnBackAndCatcht.hs` | B1–B11: `onBack`/`undo`/`retry`; X6c–X6e: `catcht` | `tests/state_react_back.rs`, `tests/exceptions.rs` |
| `LogFormat.hs` | format of the log as `logged` writes it (12 values of different types), the resulting path hash (148057647) and the text of a `NodeMSG` on the network | `transient-core/tests/log.rs` |
| `AtomFormat.hs`, `AtomFormatEdgeCases.hs` | atoms (integers, `Double`, `Float`, `Bool`, `()`, `Char`, `String`, `Text`) and reading of edge cases, **before the fix** of `Loggable.hs` | `transient-loggable/tests/golden.rs` |
| `StringFormat.hs` | strings, `Text`, `Char` and lists **after the fix**: what it writes and the round trip | `transient-loggable/tests/golden.rs` |
| `Indeterminism.hs` | `for`, `choose`, `group`, `sync`, `sync1`, `timeout`, `groupByTime`, and `oneThread`/`burst` with streams (F, C, G, S, T, O, B) | `transient-core/tests/indeterminism.rs` |
| `OneThread.hs` | `oneThread` over a `waitEvents` stream: a probe that shows whether the producer keeps generating events | `indeterminism.rs` (`o1_*`) |
| `BaseOps.hs`, `BaseOps2.hs` | `modifyData`, `modifyData'`, `try`, `sandbox`, indexed data, `**>`, `<***`, `<->`, `<**`, `addThreads`, `hookedThreads`, `freeThreads`, `onWaitThreads`, `openClose`, `localBack` with `abduce` | `transient-core/tests/base_ops.rs` |
| `Parse.hs`, `ParseSerial.hs` | the parser: numbers, atomicity, taking/skipping, combinators, streams by chunks and the applicative in serial mode | `transient-core/tests/parse.rs` |
| `LocalBack.hs` | B5b–B5c: `localBack` does not empty the stack on entry, it only restores on exit | `tests/state_react_back.rs` (`local_back_*`) |
| `SelfRunAt.hs` | `runAt` from a node to itself: closure names (29644030, 570625483 after hash 30847) and the fragment of the second one (`u/10/`); output in `SelfRunAt.output-original.txt` | `transient-core/tests/run_at.rs` |
| `AppWait.hs`, `AppThree.hs`, `AppRestart.hs` (in `transient-universe/tests/`) | the applicative of remote terms, in any order of termination, and a job after it surviving a restart (battery scenarios `appwait`, `appthree`, `apprestart`) | `transient-core/tests/applicative.rs` (processes, `examples/node_demo.rs`) |
| `WebPost.hs`, `MultiUser.hs`, `AppMinput.hs` (in `transient-universe/tests/`) | `minput`: the responses of the REST endpoints (chunked JSON, `$type` placeholders in the URL, POST body, tuples and lists, cookies, publish across sessions, a restart) captured with curl | `transient-core/tests/web.rs` (processes, `examples/web_demo.rs`) |
| `SeqFail-wire-capture.txt` | the bytes exchanged between two Haskell nodes (handshake, framing, messages), captured with a TCP proxy while running `tests/SeqFail.hs` | `transient-core/tests/wire.rs`, `two_nodes.rs` |

## `catcht`

The stored outputs of `X6` (`Exceptions.hs`) and `X6c`/`X6d`/`X6e` (`OnBackAndCatcht.hs`) were regenerated after
`catcht` was fixed in Haskell (see the last table): they show `recovered ...`, as the Rust port does.

## When `keep'` returns after an exception (it is not a failure: it is its design)

`keep'` waits for all the threads to end and does not return while any is alive or any listener remains; to wait for a
limited time there is `keepCollect`. The experiments wrap `keep'` in a 2 s `System.Timeout.timeout`: the line
`(keep' no termina)` only means that it **did not return in 2 s, that is, that some thread or listener was still alive**.

What is observed (`Exceptions.hs` run six times): when the exception reaches the first handler of
`keep'` (the one that prints it), `keep'` often does not return, and in some scenarios it sometimes does and sometimes
does not without changing anything:

    X1  no-devuelve in the 6      X8   no-devuelve in the 6      X14  no-devuelve in the 6
    X10 no-devuelve in the 6      X15  no-devuelve in the 6
    X2  returned 1 of 6           X3   returned 1 of 6           X4   returned 1 of 6
    X7  returned 1 of 6           X7b  returned 2 of 6           X9   returned 3 of 6
    X11 returned 1 of 6

When a handler stops the backtracking (`continue` in X5, `cutExceptions` in X12, `empty` in X13) or there is no
exception (X6b), it always returns. Not investigated: it is not known which node stays alive in the cases where it does not return nor
why it varies between runs. So the `*.output-original.txt` outputs are not good for checking whether `keep'` returned
in those scenarios; the lines printed by the handlers are. The Rust port always returns,
because a thread that dies of an exception is always removed from the tree.

## Bugs of `Loggable.hs` found with these experiments and fixed

The `*.output-original.txt` output of `AtomFormat*` is that of the code **before** the fix; `StringFormat`
shows the result after it.

| Bug of the original | Fix |
|---|---|
| `String` with `ñ` or `€` was written in truncated Latin-1 (`€` became one byte): information was lost | strings are JSON literals in UTF-8 |
| `Text` with `/` did not double the slash and broke the log | same as `String`: doubled slashes |
| the empty string was written `[]` and reading it gave the text `"[]"` | the empty one is `""`, different from `"[]"` |
| a list of strings wrote `b//c` and reading it returned `b//c` | the elements are JSON literals: correct round trip |
| the character `'/'` was written `'/'` and could not be read | it is written `'\47'` |
| reading the character `'"'` crashed the program (`Data.ByteString.Lazy.tail: empty`) | own reader of `Char` |

Tolerant reading: if what is read as a string does not start with quotes, it is interpreted in the old format (raw,
with `//`), so that what is written by hand (console, URL) and the previous logs can still be read. It is never written like that.
Structures (user types, tuples, `Maybe`...) still use `Show`/`Read` for now; the agreement is to move them to
JSON on both sides (pending). `FragmentType` is already JSON (see the last table).

## `oneThread`: fixed in Haskell (2026-10-26)

The original `oneThread` filtered the children of its state by thread identifier (`killChildren1`), and that failed in
both directions:

* with `waitEvents` / `parallel`, the producer of the stream is a child of the state and its thread is not that of the current event: it
  killed it, and the stream stopped after the first event (`OneThread.output-original.txt`: `produce 1`, `evento 1`,
  `proceso 1` and nothing more);
* with `option` / `react`, all the events are run by the same thread (the console one), so the filter excluded them
  all and **killed none** (`OneThreadOption.output-original.txt`: the three `proceso` appear).

Now `oneThread` kills what hangs from its state **off the path** of the current thread (`killOffPath` in `Core.hs`):
the branches of the previous results. The producer and the current result are not touched.
`OneThread.hs` and `OneThreadFixed.hs` with `waitEvents`; `OneThreadOption.hs` with `option`: the original gives 3
`proceso` and the fixed one 1 (that of the last event). `killChildren1` has not been touched: `collect` uses it.


## `onWaitThreads` and `threads` (findings when porting `BaseOps`)

* **`onWaitThreads` ran when the first thread ended**, not when all are stopped as its documentation says
  (`BaseOps2.output-original.txt`, T4: `work 1`, `WAITTHREADS`, `work 2`, `work 3`). Its check, `noactiveth`, always
  returned `True` (each child without children counted as inactive, even if it was running). **Fixed in Haskell**
  (`Core.hs`, `noactiveth`: a thread counts as active if its state is `Alive`): now it gives `work 1`, `work 2`, `work 3`,
  `WAITTHREADS` (`BaseOps2.output-fixed.txt`), as in Rust.
* **`threads`, `anyThreads` and `freeThreads` are `p <*** restore`**: the rest of the program, including what runs in the
  threads that `p` launches, goes back to the previous configuration. That is why `addThreads` inside the continuation has no effect
  (T1, peak 1). The port had a bug: the limit leaked to the rest of the program. Fixed and with tests.
* `localBack` (and therefore `sandboxData`, `sandbox'`) also restores in the calling thread even if the body goes on in another
  thread (L1): before an alternative `<|>` the handlers inside no longer exist.

## `Parse.double` has bugs (fixed in Haskell, see the last table)

`Parse.hs` (`double`) failed in these cases (see `Parse.output-original.txt`): `"-0.5"` gave `0.5` (it lost the sign when the
integer part is 0, because `integer` reads `-0` as `0`); `"1e2"`, `"1.5e2"` and `"1e-2"` **threw an exception**
(`Negative exponent`, from `10 ^ (length of the fraction - exponent)` with a negative exponent); and `"3."` crashed with
`Maybe.fromJust: Nothing`. The Rust port reads all of them correctly (`parse.rs`, with tests). In the original only the sign and the exponent
worked when the integer part is not zero and the exponent is small (`"2E1"`).

## Bugs fixed in Haskell (2026-10-27)

| What | Before | Now | Experiment |
|---|---|---|---|
| `Parse.double` | `-0.5` gave `0.5`; `1e2`, `1.5e2`, `1e-2` threw `Negative exponent`; `3.` crashed with `fromJust` | reads them well; `3.` gives `3` and leaves the dot | `Parse.hs` (original output in `Parse.output-original.txt`) |
| `catcht` | did not catch what its first argument threw (only `throwt`) | catches it, with the state in which the handler was registered | `OnBackAndCatcht.hs`, `Exceptions.hs` (X6, X6c) |
| `whileException` | never retried (it uses `catcht`) | retries until it succeeds | `Exceptions.hs`/`W1` |
| `burst` | never emitted `SDone`: `<|>` does not run the second branch if the first gives a result | uses `<->`: `SMore` per result and a `SDone` after the silence | `BurstWhile.hs` |
| `tests/flow.hs` | did not compile (`atRemote'` ambiguous) | hides the one from `Internals` | — |
| `FragmentType` serialization | `Fragment {unFragment = "..."}` (Haskell `show`/`read`, verbose) | JSON: a fragment is a JSON string with doubled slashes, `Collect` an array, `Jump` an object; also in the TCache files of the closures | `SeqFail-wire-capture.txt` |
| `Web.hs` cookies | `Set-Cookie` was written after the blank line that ends the headers, as the first bytes of the body, where no client reads it as a cookie (the scripts read it with `curl --raw`) | the headers are left open: the cookie goes inside them and the first thing that is not a header (or the end of the cookies) closes them (`openHeaders`, `closeHeaders` in `Internals.hs`). A response with nothing sent yet shows no end of headers. The scripts `mur.sh`, `mu2.sh` and `runcol.sh` read the cookie with `curl -i` | `WebPost.hs`, `MultiUser.hs`; Rust `tests/web.rs` |
| `Web.hs` `getURL` | never gave a result: it asked the state for a `Maybe PrevClos` (`tryState`), which is never there | reads the `PrevClos` with `getData`; the session is printed without the quotes of `show` | `ShowUrl.hs` |
