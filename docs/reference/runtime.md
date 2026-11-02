# Idle stop, memory and the engine cache

These settings tune how `start` runs a long-lived app. Each has a key in `ampoule.toml` and a run
option that overrides it.

## Stopping when idle

With `--idle-stop TIME` (or `AMPOULE_IDLE_STOP`, or `idle_stop` in `ampoule.toml`), `start` stops
the VM after `TIME` without connections and starts it again when the next one arrives, so an idle
app costs only the runtime's own process:

```console
$ ./my_app --idle-stop 5m start
ampoule: stopping the VM after 5m without connections
...
ampoule: no connections for 5m, stopping the VM until the next one
ampoule: VM stopped; waiting for a connection on port 4000
ampoule: connection on port 4000, starting the VM
```

The process that runs `start` holds the app's listening sockets and runs the VM in a child process.
When the VM stops, the sockets stay open: a new connection waits in the listen backlog while the
VM boots, and is accepted as soon as the app listens again, so none is refused. With `serve`, the
earlier steps run once and only `start` runs again.

| Realtime, measured | Idle | Waking |
|---|---|---|
| macOS, aarch64 | 2 MB, against about 150-250 MB running | first response 0.8 s after a request on a stopped VM |
| Linux, aarch64 (francis) | 1.6 MB | first response 0.3 s |

- Only connections to ports the app listens on with a fixed number count; outgoing ones (a
  database, an API) do not. An open connection, such as a WebSocket, keeps the VM running.
- Background work stops with the VM: timers, scheduled jobs, outgoing connections, subscriptions.
  Use it for apps that only work in response to connections.
- SIGTERM and SIGINT go to the running VM as usual; with the VM stopped, the process exits at once.
- A connection that arrives at the moment the VM decides to stop can be accepted by an app that is
  shutting down, and dropped.

## Memory and restarts

The VM's memory is capped: by `--memory-max`, or at 4 GiB, the most wasm32 can address. As on a
native release, an allocation the VM cannot make ends it with a crash dump (`binary_alloc: Cannot
allocate ...`). ampoule gives the app two ways to deal with that: an alarm before it happens, and a
restart after.

### The memory alarm

`start` watches `:erlang.memory(:total)` against the limit and sets OTP's standard
`{:system_memory_high_watermark, []}` alarm through `:alarm_handler` at 80% (`memory_alarm`,
`--memory-alarm`), clearing it 10 points lower. It is the alarm os_mon's memsup raises, which cannot
run here, so code that already sheds load on memsup alarms works unchanged:

```elixir
:gen_event.add_handler(:alarm_handler, MyApp.MemoryAlarm, [])   # handle_event({:set_alarm, ...})
```

The watch looks twice a second once memory is past half the alarm level, and every 5 seconds below
that, so an idle VM spends next to nothing on it; while memory is below half the alarm level, the
alarm can come up to 5 seconds late. It catches memory that grows, not a single allocation that
jumps past the limit. `/proc/meminfo` reports the limit as `MemTotal`, as a container's cgroup limit
shows, for code that sizes itself from it.

### Restarting

With `restart = "on-failure"` (or `--restart on-failure`), `start` runs the VM as a child and starts
it again when it exits with a non-zero status, after 1, 2, 4, 8 and 16 seconds. Listening sockets
stay open in between (as with [`--idle-stop`](#stopping-when-idle)), so clients wait instead of
being refused. After 5 failures within 5 minutes it gives up and exits with the VM's status, for
systemd, Kubernetes or Docker to take over. A clean stop (`System.stop/0`, SIGTERM) is not
restarted. Each failure leaves its crash dump in the run's `/tmp`.

```console
$ ./my_app --memory-max 256M --restart on-failure start
[warning] ampoule: VM memory at 81% of its 256 MiB limit; alarm system_memory_high_watermark set
binary_alloc: Cannot allocate 16777216 bytes of memory (of type "binary").
ampoule: VM failed (status 1); restarting in 1s (1 of 5 in 5m)
```

Usable memory is less than the limit: thread stacks and allocator overhead live in the same memory,
more of it with more schedulers. To kill a runaway process rather than the VM, cap process heaps
with `+hmax` (and `+hmaxib true` to count binaries) in `vm_args`.

### After a spike

Memory the VM frees goes back to the host, so the process's RSS falls again once a spike is over:
when ERTS releases a carrier, the runtime drops its pages, keeping at most 32 MiB of them resident
for the next carriers to reuse. ERTS's own cache of freed carriers is off (`+MMmcs 0`): it holds
them per scheduler, where under a memory cap they make allocations fail that the free memory would
serve. Setting `+MMmcs` in `vm_args` turns it back on.

What stays is the VM's address space. Wasm memory cannot shrink, so the range a spike reached
remains the VM's, reused for its later allocations without taking host memory, and `--memory-max`
still caps it. Memory from `malloc` (NIFs, OpenSSL, thread stacks) is reused by the VM but not
returned.

## The engine cache

The binary carries the engine (ERTS compiled to native code) compressed. The first start writes it
out decompressed, and every start after that maps it from that file. Its pages are clean and
shared, so the kernel can drop them under pressure and VMs of the same binary share them, where
decompressing into memory kept about 16 MiB private to each VM.

The file goes in the first of these that is set:

1. `AMPOULE_NATIVE_CACHE`
2. systemd's `CACHE_DIRECTORY` (a unit with `CacheDirectory=`; `<dir>/ampoule`)
3. `$XDG_CACHE_HOME/ampoule/native` (an absolute `XDG_CACHE_HOME` only)
4. `~/.cache/ampoule/native`

Each engine (one per ampoule version, and per app with NIFs) takes about 17 MiB, and the four most
recently used are kept. The file is named and checked by the SHA-256 of the code the binary
carries, so a damaged or replaced file is never run but written again. The directory and file must
belong to the user running the app, with no group or other write permission.

When the directory cannot be written (a read-only file system) or refuses executable mappings (a
`noexec` mount), the VM loads the engine in memory and says why on stderr. After a failed mapping,
a `<hash>.unusable` file in the directory stops later starts from trying again until it is removed.
`AMPOULE_NO_CACHE=1` turns the cache off.

The app chooses when it is packed, with `native_cache` in `ampoule.toml`; `--native-cache` or
`AMPOULE_NATIVE_CACHE` overrides it where it runs:

- `auto` (the default): the directories above, the first that is set.
- `off`: nothing is written, ever; every start loads the engine in memory. For a board whose
  storage should not wear, or a deployment that must not leave files behind. The binary is the
  same single file either way: the cache is only ever made from what it carries.
- a directory: that one. Several instances of the same binary on one host pointing at the same
  directory map one copy of the engine between them, instead of one each: in a test of 1000 nodes
  in containers, about 17 MiB a node instead of 50.

Never mount the cache directory, or a directory containing it, read-write into an app (`--mount`):
the app could change the engine file after it was checked, and another VM mapping it would run
that change as native code, outside wasm. A VM whose own read-write mounts (or `--tmp`) contain
the cache directory does not use it, and says so.
