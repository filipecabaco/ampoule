# iPhone and iPad

An iOS app can start no other process, and a device maps no executable memory, so there is no
binary to pack for iOS. The app links ERTS in instead and boots the release on a thread of its
own. `--target ios` writes what it bundles:

```sh
ampoule release --target ios
# _build/prod/ampoule/my_app-ios/
#   release/                the release, pruned and stripped, without ERTS or host native code
#   liberlang.xcframework   ERTS, OTP's crypto and asn1 NIFs and OpenSSL, for devices and the simulator
#   runtime.json            the OTP, ERTS and minimum iOS it was built for
```

The app calls `erl_start(argc, argv)` with the release's boot script, as its start script would.
[ex_tauri](https://github.com/filipecabaco/ex_tauri)'s `mix ex_tauri.ios device` does all of it
for a Tauri app.

The runtime comes with the macOS release archives (`ios/`, next to `ampoule`), or from
`AMPOULE_IOS_RUNTIME`; `mise run ios` builds it from source. Only OTP's own native code runs there:
a release with other NIFs is refused unless `--allow-native`, which leaves them out.
