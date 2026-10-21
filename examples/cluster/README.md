# Cluster

An OTP app whose nodes find each other with Erlang distribution, as one binary, without epmd.
Each node runs a member of the `:roster` process group (`:pg`), so once the nodes are connected,
each sees all of them.

```sh
ampoule release
# on 10.0.0.5
RELEASE_COOKIE=... _build/prod/ampoule/roster-<target> --node roster@10.0.0.5 start
# on 10.0.0.6
RELEASE_COOKIE=... ROSTER_PEERS=roster@10.0.0.5 _build/prod/ampoule/roster-<target> --node roster@10.0.0.6 start
# from either machine
RELEASE_COOKIE=... _build/prod/ampoule/roster-<target> --node roster@10.0.0.5 rpc 'IO.inspect(Roster.members())'
# [:"roster@10.0.0.5", :"roster@10.0.0.6"]
```

- `ampoule.toml`: `[distribution]` with long names and no epmd. Every node listens on port 9100
  and reaches its peers on 9100, so the port is all a node needs to know. `--node` (or
  `RELEASE_NODE`) names each node.
- `lib/roster.ex`: `Roster.Peers` connects to the nodes in `ROSTER_PEERS` and again when a
  connection drops; `Roster.members/0` lists the nodes in the group.

Two nodes on one machine cannot both have port 9100. Give each its own host port with
`--port-map 9100:9101`, and send the other's 9100 there with
`--rewrite-connect 127.0.0.1:9100=127.0.0.1:9102`; the tests run them this way.

## Tests

`mix test` packs the app and starts two nodes from the binary (`test/e2e/roster_test.exs`): they
connect and see each other's members, calls go both ways, `rpc` runs on a node with its
environment, this test's own VM joins as a native hidden node and the packed node calls it back,
a cookie from a file works and a wrong one is refused, and `remote` opens a shell on a running node.
