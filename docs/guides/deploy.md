# Deploying

A packed binary is one file. Copy it to a machine of the right OS and CPU and run it.

## To a server

The Linux binaries are statically linked, so any Linux machine of the right architecture runs them:

```sh
scp _build/prod/ampoule/my_app-x86_64-linux server:
ssh server 'DATABASE_URL=... PORT=8080 ./my_app-x86_64-linux serve'
```

Under systemd, `systemctl stop` sends SIGTERM, which stops the app through `init:stop()`. A unit
with `CacheDirectory=` gets the runtime's decompressed engine in that directory instead of
`~/.cache` (see [The engine cache](../reference/runtime.md#the-engine-cache)).

## In an empty container

The binary is the only file the image needs:

```dockerfile
FROM scratch
COPY my_app-aarch64-linux /my_app
ENTRYPOINT ["/my_app"]
CMD ["start"]
```

`docker stop` stops the app through `init:stop()`, the same as a native release. If the app makes
TLS connections, give it CA certificates as well, for example `COPY --from=alpine:3.22
/etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/`. A `FROM scratch` image has no programs, not
even `/bin/sh`, so an app that starts [host programs](../reference/sandbox.md#host-programs) needs
them in the image.

## To another platform

Packing for another platform needs that platform's lean runtime, `ampoule-<target>`, next to
`ampoule`. The [install script](../getting-started.md#install) puts every platform's there, so
naming the target is enough:

```sh
ampoule release --target x86_64-linux
```

After a manual install, download the one you need from the same release first:

```sh
curl -fLo ~/ampoule/ampoule-x86_64-linux https://github.com/filipecabaco/ampoule/releases/latest/download/ampoule-x86_64-linux
```

`--target` repeats, and `AMPOULE_STUBS` points at a directory holding several runtimes:

```sh
AMPOULE_STUBS=~/ampoule-stubs ampoule release --target aarch64-darwin --target x86_64-linux
```

NIFs are rebuilt for wasm, so a Mac can pack the Linux binary for a Docker image, NIFs included.

## Limits to plan for

- Memory: at most 4 GiB per VM; `--memory-max` caps it lower. See [Memory and
  restarts](../reference/runtime.md#memory-and-restarts).
- `:erlang.system_info(:logical_processors)` is 1, but scheduler counts are resolved against the
  host's cores.
- A clustered app starts no epmd; see [Clustering](cluster.md).

[Limitations](../limitations.md) has the full list.
