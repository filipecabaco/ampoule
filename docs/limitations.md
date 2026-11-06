# Limitations

- Speed: the VM runs as an interpreter. WebAssembly cannot hold the JIT that a native OTP uses
  (`:erlang.system_info(:emu_flavor)` is `:emu`, where a native release says `:jit`), so CPU-bound
  code is slower than in a native release. Measured on Apple silicon (M5 Pro, OTP 28.5, ampoule
  0.1.13), as compiled code in a plain `mix release` of the same app and in the packed binary:

  | Work | Native release | ampoule | |
  |---|---|---|---|
  | tight integer loop, 100M iterations | 125 ms | 2.56 s | 20x |
  | `Enum.reduce` over a 5M range | 9 ms | 119 ms | 13x |
  | `fib(32)` | 8 ms | 48 ms | 5.8x |
  | 50k SHA-256 hashes of 4 KB | 68 ms | 414 ms | 6.1x |
  | 20k JSON round trips | 14 ms | 52 ms | 3.8x |
  | sort 1M integers | 303 ms | 1.08 s | 3.6x |
  | 500k ETS inserts and lookups | 160 ms | 242 ms | 1.5x |

  Part of that is the JIT against any interpreter: a native OTP built with `--disable-jit` is 3.2
  to 3.6x slower than the JIT on `fib(32)` and the integer loop. The rest is the interpreter
  running as WebAssembly, which costs 1.4 to 1.6x on code that spends its time in allocation,
  GC and `fib`-style calls, and 6.4x on the tight arithmetic loop. Code that mostly waits on I/O
  loses much less. `examples/francis` (Bandit, a small HTML route) served 62,500 requests per
  second over 50 keep-alive connections against 82,000 natively, and 9,200 against 17,300 over
  one. Starting to the first response took 255 ms against 373 ms, and the process used about
  100 MiB of memory against about 200 MiB (the native figure includes one scheduler thread per
  core). These are one machine and a few workloads: measure yours.
- Host programs run only when [allowed](reference/sandbox.md#host-programs). Programs in the
  release's own `priv/` run as they were built: one compiled on the machine that packed runs only
  on hosts of the same OS and CPU, and packing warns about the others.
- os_mon's `memsup`, `cpu_sup`, `disksup` and `os_sup` stay switched off, since their programs
  would run on the host and report on it rather than on the VM. The [memory
  alarm](reference/runtime.md#the-memory-alarm) stands in for `memsup`'s.
- Memory: at most 4 GiB. Freed memory goes back to the host, but the VM keeps the address space a
  spike reached (see [After a spike](reference/runtime.md#after-a-spike)).
- File permissions: files the app creates start at 0644 (directories 0755), less the host's umask,
  as on most hosts natively; `File.chmod/2` then sets the mode on the host file. It cannot set
  setuid or setgid bits, and the owner (`File.stat/1`'s `uid`, `gid`) always reads 0.
- Identity: `:os.type()` is `{:unix, :linux}` and `:erlang.system_info(:logical_processors)` is 1
  on every host. `System.pid/0` is the host pid of the process running the VM (the child, under
  `--restart` or `--idle-stop`).
- Distribution: no epmd is started, so without the host's epmd every node of a cluster uses the
  same distribution port, one node per address (see [Clustering](guides/cluster.md)). Clustering
  was tested over TCP (`inet_tcp_dist`) on one machine; TLS distribution was not.
- NIF threads: a thread a NIF starts must return from its function. `enif_thread_exit` ends the
  thread without the cleanup wasi-libc would do, so joining that thread never returns.
- OTP version: the engine fixes the OTP major, currently OTP 28 (ERTS 16).
- OTP's `socket` and `net` modules are not in the engine; `gen_tcp`, `gen_udp` and `inet` are.
- Precompressed static files: packing drops `.gz` and `.br` files, along with sources, docs, `.map`
  files, host libraries and shell scripts outside `priv/`. `Plug.Static` falls back to the
  uncompressed file.
- Debug information: `.beam` files lose their `Dbgi`, `Docs`, `ExCk`, `ExDp` and `Abst` chunks, so
  `Code.fetch_docs/1` and the debugger's source views have nothing to show.
- Shutdown: a clean stop (SIGTERM, `System.stop/0`) takes about a second, as it does natively
  (1.04 s against 1.05 s for a Bandit app on Apple silicon), because kernel's `user_sup` waits
  1000 ms before it stops the user process. `eval`, `[commands]` entries and `main` end with
  `halt()` and do not wait.
