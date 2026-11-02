# Clustering

`start` runs the app as one node unless distribution is configured. Naming a node turns it on:

```console
$ RELEASE_COOKIE=... ./my_app --node my_app@10.0.0.5 start
$ RELEASE_COOKIE=... ./my_app --node my_app@10.0.0.5 rpc 'IO.inspect(Node.list())'
[:"my_app@10.0.0.6"]
```

The node is `-name` when its host has a dot or a colon (an FQDN or an IP address) and `-sname`
otherwise. `--distribution` (`RELEASE_DISTRIBUTION`, `mode`) says which, as with `mix release`, and
`none` turns distribution off. `eval` and custom commands run without distribution, as a native
release's `eval` does. The node sees `RELEASE_NODE` and `RELEASE_DISTRIBUTION` in its environment.

The same settings can be packed into the binary:

```toml
[distribution]
mode = "name"   # long names; give each node its own with --node or RELEASE_NODE
port = 9100     # every node listens here and reaches its peers here
epmd = "none"
```

[`examples/cluster`](../../examples/cluster) is an app whose nodes find each other this way, from
one binary, on two machines or on one.

## Finding peers

The runtime starts no epmd. There are two ways to find the other nodes.

Without epmd (`epmd = "none"`, the default), every node listens on the distribution port, 9100
unless `port` or `--dist-port` says otherwise, and reaches its peers on that same port. This is
OTP's `-erl_epmd_port`, which `mix release` documents for deploying without epmd; a release whose
`vm.args` sets `-erl_epmd_port` gets that port. A native node joins with the same flags:
`erl -name x@10.0.0.7 -setcookie ... -start_epmd false -erl_epmd_port 9100`.

With the host's epmd (`epmd = "host"`), the node registers with an epmd already running on the
host (`127.0.0.1:4369`, or `ERL_EPMD_PORT`), as a native release does, on a free port or on
`port`. ampoule does not start an epmd, and a machine without Erlang has none to start.

## Several nodes on one machine

Without epmd, two nodes on one address cannot both have the port. On one machine, give each node
its own host port and send the other's there:

```sh
./my_app --node a@127.0.0.1 --port-map 9100:9101 --rewrite-connect 127.0.0.1:9100=127.0.0.1:9102 start
./my_app --node b@127.0.0.1 --port-map 9100:9102 --rewrite-connect 127.0.0.1:9100=127.0.0.1:9101 start
./my_app --node a@127.0.0.1 --dist-port 9101 rpc 'IO.inspect(Node.connect(:"b@127.0.0.1"))'
```

## The cookie

The cookie comes from `--cookie-file` (`AMPOULE_COOKIE_FILE`), then `RELEASE_COOKIE`, then
`cookie_file` in `ampoule.toml`, then the release's own `releases/COOKIE`, as with `mix release`.
That last one is generated when the release is built and packed into the binary, so anyone with
the binary has it: set one of the others in production. The cookie reaches the VM on its command
line inside the runtime, which the host's `ps` does not show, and ampoule never prints it.

## The network policy

The network policy applies to distribution as to any socket. The distribution port is allowed even
when `--listen` leaves it out, `--port-map` maps it (except with `epmd = "host"`, where epmd would
give peers the app's port), and connections to peers go through `--rewrite-connect` and
`--deny-connect`.

Node names must resolve inside the app. An IP address works as it is. The host's own name (what
`-sname` uses) is added to the app's `/etc/hosts` as `127.0.0.1` when the host's file does not
have it, and other names come from the host's `/etc/hosts`, `--host` and DNS.

## `remote` and `rpc`

`remote` and `rpc` take the same options as the node and connect to it: they start a hidden VM
from the same binary that does not listen (`-dist_listen false`), with the release's
`remote.vm.args`. When the node's port is mapped, give them the host port with `--dist-port`. As
with a native release, a remote shell whose input ends stops the node it is attached to; leave it
with Ctrl-C.

## VM flags

In the release's `vm.args`, `-name`, `-sname` and `-setcookie` are left out with a warning, and
`-hidden`, `-proto_dist`, `-epmd_module` and `-dist_listen` apply.

## Tested

Clustering was tested over TCP (`inet_tcp_dist`) on one machine; TLS distribution was not.
