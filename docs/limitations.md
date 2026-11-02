# Limitations

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
- Shutdown: a clean stop (SIGTERM, `System.stop/0`) takes about a second longer than it does
  natively, because kernel's `user_sup` waits 1000 ms before it stops the user process.
