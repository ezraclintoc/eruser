# Porting Go to Rust

Notes for anyone continuing the port, or reviewing what the AI pass produced.
These are the rules the existing modules were written against, so following
them keeps the codebase consistent.

The short version: **translate the behaviour, not the shape.** A faithful port
is one where the program does the same thing, not one where the Rust reads
like Go with different punctuation.

---

## The one rule that matters most

Go and Rust disagree about what a type can promise. Go leans on zero values,
nil, and "check the second return"; Rust makes the compiler carry those
guarantees instead. When you port a function, ask what invariant the Go code
was holding in its head, and write it into a type.

That's where the real bugs get found. Every difference listed in the commit
messages came out of asking that question.

---

## Mechanical translations

| Go | Rust | Notes |
|---|---|---|
| `error` as second return | `Result<T, E>` | Never a tuple. |
| `fmt.Errorf("...: %w", err)` | `thiserror` enum with `#[source]` | One variant per distinguishable failure. |
| `nil` pointer meaning "absent" | `Option<T>` | Do not use a sentinel. |
| `nil` pointer into a slice | `Option<&T>` or an owned value | See below. |
| `interface{ Do() }` | a trait | Add `#[async_trait]` only if it must be `dyn`. |
| `map[string]bool` as a set | `HashSet<String>` | |
| `struct` tags for YAML | `#[serde(...)]` | Keep the on-disk names identical. |
| `sync.Mutex` around a field | put the data *inside* `Mutex<T>` | The lock should own what it protects. |
| `context.Context` for cancellation | `CancellationToken` | Await it in `select!`, don't poll it. |
| `context.WithValue` | an explicit parameter | Typed arguments, not a string-keyed bag. |
| `time.Time` zero value | `Option<DateTime<Utc>>` | There is no "zero time"; absent is absent. |
| `defer x.Close()` | `Drop`, or an explicit `close().await` | Async cleanup can't go in `Drop`. |
| goroutine + channel | `tokio::spawn` + `mpsc`, or just `.await` | Most goroutines in this codebase were sequential anyway. |

---

## Specific traps in this codebase

### Returning pointers into a slice

Go does this constantly:

```go
func (db *BrokerDatabase) FindByID(id string) *Broker {
    for i := range db.Brokers {
        if db.Brokers[i].ID == id {
            return &db.Brokers[i]   // caller can now mutate the database
        }
    }
    return nil
}
```

The Rust is `Option<&Broker>` — and if the caller needs to mutate, that's a
separate `_mut` method, so mutation is visible at the call site. For removal,
return the owned value that came out:

```rust
pub fn remove_by_id(&mut self, id: &str) -> Option<Broker>
```

Go's version returned a pointer to an element it had just shifted out of the
slice.

### The empty string is not the same as absent

Go's YAML structs use `string` plus `omitempty`. Keep that on disk — existing
config files must still load — but be honest inside the program. Where the
distinction matters, use `Option<String>`; where it genuinely doesn't (an
optional address line that renders as nothing), a `String` with a
`skip_serializing_if` is fine and less noisy.

### Silent zero values

`SUM()` over zero rows is `NULL`, not `0`. `time.Parse` failing leaves a zero
`time.Time`. A missing template field renders as `<no value>`. Go swallowed
all three. Each is now either an `Option` or an error — see the `history` and
`template` modules.

### Errors that get shown to people

Two rules, both learned from real problems in the original:

1. **Don't leak.** Transport errors from `lettre` include the server's
   response, which some providers echo the username back in. `email::classify`
   maps them onto an enum and drops the raw text.
2. **Don't over-summarise.** Rust errors nest, and `to_string()` only prints
   the top level. A history row reading "invalid recipient address" with no
   address in it is useless. Use `send::error_chain` when writing an error
   somewhere a human will read it later.

### Duplicated logic

The send loop existed twice in Go — once for the CLI, once for the web UI —
and the two had already drifted. If you find yourself porting the same logic
into two places, extract it and give the callers a callback instead. `send.rs`
is the pattern: one pipeline, a `Progress` enum, and each front end decides
how to display it.

---

## Async

The rules of thumb:

- Anything doing I/O over a network is `async`.
- File and config I/O is synchronous. It happens once at startup and blocking
  briefly there is not worth the colour-of-function tax.
- Use `#[async_trait::async_trait]` only where the trait must be `dyn`-safe,
  which in practice means `Sender`.
- Await a `CancellationToken` inside `select!` alongside whatever you're
  waiting on. A cancel that only takes effect after a 2-second sleep is a
  cancel the user notices is broken.

---

## Tests

Every module lands with tests in the same commit. Not later.

- Tests go in `src/<module>/tests.rs`, declared with `#[cfg(test)] mod tests;`.
- **Name the behaviour, not the function.**
  `filter_excludes_by_id_and_by_name_case_insensitively`, not `test_filter`.
  The name should tell you what broke without opening the file.
- **Never touch the network.** `Sender` is a trait precisely so the send
  pipeline can be tested against a recording fake. If something is hard to
  test, that is usually the code telling you it needs a seam.
- **Test the wording.** Anything a person reads — CLI output, error messages —
  gets a test. They are the part users actually experience, and they rot
  silently.
- **Pin the reasons.** When a port fixes something the original got wrong,
  write a test with a comment saying what it was. That's what stops the next
  person from "simplifying" it back.
- Use `#[tokio::test(start_paused = true)]` for anything involving time.
  Tests must not actually sleep.

---

## Style

- `rustfmt` and `clippy -D warnings` both pass. CI enforces it.
- Comments explain *why*. The code already says what.
- Doc comments on public items, especially where behaviour differs from
  upstream — that's the note a future reader needs most.
- No `unwrap()` outside tests. `expect()` is acceptable when the invariant is
  provable and the message says what it is.

---

## Commits

One module per commit, with tests. The body says what changed relative to the
Go version and why — not just what the Rust does. Those commit messages are
the real record of what this port decided, and they're worth writing properly.
