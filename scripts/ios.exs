#!/usr/bin/env elixir

# Build the iOS runtime: ERTS as a static library for iPhone/iPad and the Apple Silicon simulator
# (build/ios/liberlang.xcframework), which `ampoule pack --target ios` pairs with a release.
#
#   elixir scripts/ios.exs [STEP...] [--build DIR] [--jobs N] [--force]
#
# Steps, run in this order (default: all of them):
# (downloads are pinned and SHA-256 checked, the same as scripts/engine.exs)
#
#   sources    download OTP and OpenSSL
#   openssl    build OpenSSL for each slice (libcrypto.a)
#   erts       build ERTS for each slice from a fresh, patched OTP tree (engine/patches/erts-ios.patch)
#   framework  join the slices into build/ios/liberlang.xcframework, with runtime.json
#
# Why native ERTS and not the wasm engine: a device forbids starting a child process and mapping
# executable memory, and wasmtime's interpreter (Pulley) has no shared memory, which the engine's
# schedulers need. ERTS's own interpreter, linked into the app as ordinary signed code, needs
# neither: the app calls erl_start/2 on a thread of its own.
#
# The patch: OTP compiles its static crypto NIF without what configure found (@SSL_FLAGS@,
# @DEFS@), which the shared one gets through CFLAGS. Without HAVE_OPENSSL_CRYPTO_MEMCMP,
# crypto:hash_equals/2 answers notsup, and every TLS record then fails to decode
# ("malformed_handshake_data").
#
# Needs macOS with Xcode, and a host Erlang/OTP of the same major on PATH (`mise install`).

Mix.install([{:req, "~> 0.7"}])

defmodule Ios do
  @root Path.expand("..", __DIR__)
  @steps ~w(sources openssl erts framework)

  # The same sources as scripts/engine.exs: a release's OTP must match the runtime it runs on.
  @otp "28.5.0.4"
  @otp_sha256 "efb045f96ee56d274f6c1fe3a9612b45caa01d2f82e35fe4e0ac9b0c501f6e53"
  @openssl "3.5.9"
  @openssl_sha256 "603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a"

  # The oldest iOS the slices run on; an app may require a newer one, not an older one.
  @min_ios "15.0"

  @slices [
    %{id: "ios-arm64", xcomp: "arm64-ios", openssl: "ios64-xcrun", host: "aarch64-apple-ios", min: "-mios-version-min=#{@min_ios}"},
    %{
      id: "ios-arm64-simulator",
      xcomp: "arm64-iossimulator",
      openssl: "iossimulator-arm64-xcrun",
      host: "aarch64-apple-iossimulator",
      min: "-mios-simulator-version-min=#{@min_ios}"
    }
  ]

  @openssl_options ~w(no-shared no-tests no-apps no-docs no-dso no-engine no-ui-console)

  def run(argv) do
    Logger.configure(level: :info)
    {opts, steps} = OptionParser.parse!(argv, strict: [build: :string, jobs: :integer, force: :boolean])
    steps = if steps == [], do: @steps, else: steps
    for s <- steps, s not in @steps, do: fail("unknown step #{s} (steps: #{Enum.join(@steps, ", ")})")
    if not match?({:unix, :darwin}, :os.type()), do: fail("the iOS runtime builds on macOS only (it needs Xcode)")

    b = Path.expand(Keyword.get(opts, :build, Path.join(@root, "build")))

    ctx = %{
      build: b,
      jobs: Keyword.get(opts, :jobs, System.schedulers_online()),
      force: Keyword.get(opts, :force, false)
    }

    for s <- @steps, s in steps do
      step(String.to_atom(s), ctx)
    end
  end

  # ---------------------------------------------------------------------------------------------
  # Steps

  defp step(:sources, ctx) do
    tarballs = [download_path(ctx, otp_url()), download_path(ctx, openssl_url())]

    once(ctx, :ios_sources, [@otp, @openssl], tarballs, fn ->
      download(ctx, otp_url(), @otp_sha256)
      download(ctx, openssl_url(), @openssl_sha256)
    end)
  end

  defp step(:openssl, ctx) do
    for s <- @slices do
      once(ctx, :"ios_openssl_#{s.id}", [@openssl, @openssl_options, s, xcode(ctx)], [libcrypto(ctx, s)], fn ->
        src = Path.join(ios(ctx), "openssl-src-#{s.id}")
        extract(download_path(ctx, openssl_url()), src)

        cmd!(
          "perl",
          ["Configure", s.openssl | @openssl_options] ++ ["--prefix=#{openssl(ctx, s)}", "--libdir=lib", s.min],
          cd: src,
          log: log_file(ctx, "openssl-#{s.id}")
        )

        cmd!("make", ["-j#{ctx.jobs}", "build_libs"], cd: src, log: log_file(ctx, "openssl-#{s.id}"))
        cmd!("make", ["install_dev"], cd: src, log: log_file(ctx, "openssl-#{s.id}"))
        File.rm_rf!(src)
      end)
    end
  end

  defp step(:erts, ctx) do
    check_host_otp!()
    patch = Path.join(@root, "engine/patches/erts-ios.patch")

    for s <- @slices do
      inputs = [@otp, File.read!(patch), s, xcode(ctx), stamp(ctx, :"ios_openssl_#{s.id}")]

      once(ctx, :"ios_erts_#{s.id}", inputs, [slice_lib(ctx, s)], fn ->
        # Always a fresh tree: OTP's Makefiles do not track flags, and objects left from an
        # earlier configuration silently keep its feature macros.
        otp = Path.join(ios(ctx), "otp-#{s.id}")
        File.rm_rf!(otp)
        extract(download_path(ctx, otp_url()), otp)
        # Not `git apply`: inside this repository it would resolve paths against the repository root.
        cmd!("patch", ["-p1", "--forward", "--batch", "-i", patch], cd: otp)

        conf = Path.join(ios(ctx), "erl-xcomp-#{s.xcomp}.conf")

        Path.join(otp, "xcomp/erl-xcomp-#{s.xcomp}.conf")
        |> File.read!()
        |> String.replace(~r/-mios(-simulator)?-version-min=[0-9.]+/, s.min)
        |> then(&File.write!(conf, &1))

        nifs = static_nifs(otp, s)
        log("configure ERTS for #{s.id} (log: #{log_file(ctx, "erts-#{s.id}")})")

        cmd!(
          Path.join(otp, "otp_build"),
          [
            "configure",
            "--xcomp-conf=#{conf}",
            "--with-ssl=#{openssl(ctx, s)}",
            "--disable-dynamic-ssl-lib",
            "--enable-static-nifs=#{Enum.join(nifs, ",")}"
          ],
          cd: otp,
          env: [{"ERL_TOP", otp}, {"LIBS", libcrypto(ctx, s)}, {"TARGET", nil}],
          log: log_file(ctx, "erts-#{s.id}")
        )

        # Only the emulator (libbeam.a) and what it links: it builds its static NIFs itself. The
        # rest of OTP is not needed, the app bringing the release's own .beam files, and its
        # dynamic drivers do not even link for iOS (OTP passes ld a compiler flag).
        for dir <- ["erts/lib_src", "erts/emulator"] do
          log("make #{dir} for #{s.id}")

          cmd!("make", ["-j#{ctx.jobs}", "opt", "FLAVOR=emu"],
            cd: Path.join(otp, dir),
            env: [{"ERL_TOP", otp}, {"TARGET", s.host}],
            log: log_file(ctx, "erts-#{s.id}")
          )
        end

        # One archive per slice: the emulator, its support libraries, the static NIFs and OpenSSL.
        archives = slice_archives(otp, s) ++ nifs ++ [libcrypto(ctx, s)]
        log("#{s.id}: #{archives |> Enum.map(&Path.basename/1) |> Enum.sort() |> Enum.join(" ")}")
        File.mkdir_p!(Path.dirname(slice_lib(ctx, s)))
        File.rm(slice_lib(ctx, s))
        cmd!("libtool", ["-static", "-o", slice_lib(ctx, s) | archives], log: log_file(ctx, "erts-#{s.id}"))

        File.write!(Path.join(Path.dirname(slice_lib(ctx, s)), "erts_vsn"), erts_vsn(otp) <> "\n")
        File.rm_rf!(otp)
      end)
    end
  end

  defp step(:framework, ctx) do
    out = framework(ctx)
    inputs = for s <- @slices, do: stamp(ctx, :"ios_erts_#{s.id}")

    once(ctx, :ios_framework, inputs, [out, Path.join(ios(ctx), "runtime.json")], fn ->
      File.rm_rf!(out)
      libraries = Enum.flat_map(@slices, &["-library", slice_lib(ctx, &1)])
      cmd!("xcodebuild", ["-create-xcframework" | libraries] ++ ["-output", out])

      [erts | _] = for s <- @slices, uniq: true, do: slice_erts_vsn(ctx, s)

      runtime = %{
        format: 1,
        otp: @otp,
        erts_vsn: erts,
        openssl: @openssl,
        min_ios: @min_ios,
        # erl_start(argc, argv) runs the VM on the calling thread until it halts.
        entry: "erl_start",
        # Linked with every slice beside liberlang.a.
        system_libs: ["c++"]
      }

      File.write!(Path.join(ios(ctx), "runtime.json"), JSON.encode!(runtime))

      size = for s <- @slices, reduce: 0, do: (acc -> acc + File.stat!(slice_lib(ctx, s)).size)
      log("liberlang.xcframework: OTP #{@otp}, ERTS #{erts}, #{length(@slices)} slices, #{div(size, 1_048_576)} MiB")
    end)
  end

  # ---------------------------------------------------------------------------------------------
  # OTP

  # OTP's own NIFs, linked in: a device loads no shared library an app did not ship signed.
  defp static_nifs(otp, s) do
    [
      Path.join(otp, "lib/asn1/priv/lib/#{s.host}/asn1rt_nif.a"),
      Path.join(otp, "lib/crypto/priv/lib/#{s.host}/crypto.a")
    ]
  end

  # Every archive built for the slice: the emulator (libbeam.a, from RELEASE_LIBBEAM) and the
  # libraries it links. Variants (_st, _r) and anything built for the host are left out.
  defp slice_archives(otp, s) do
    {host, 0} = System.cmd(Path.join(otp, "erts/autoconf/config.guess"), [])
    host = String.trim(host)

    otp
    |> Path.join("**/*.a")
    |> Path.wildcard()
    |> Enum.filter(&String.contains?(&1, "/#{s.host}/"))
    |> Enum.reject(&(String.contains?(&1, host) or String.ends_with?(&1, ["_st.a", "_r.a"])))
    |> Enum.reject(&(&1 in static_nifs(otp, s)))
    |> Enum.uniq_by(&Path.basename/1)
  end

  defp check_host_otp! do
    want = @otp |> String.split(".") |> hd()
    have = System.otp_release()

    if have != want,
      do:
        fail("building ERTS #{@otp} needs a host Erlang/OTP #{want} on PATH, found #{have} (run through `mise exec --`)")
  end

  defp erts_vsn(otp) do
    ~r/^VSN = (.+)$/m
    |> Regex.run(File.read!(Path.join(otp, "erts/vsn.mk")), capture: :all_but_first)
    |> hd()
    |> String.trim()
  end

  defp slice_erts_vsn(ctx, s), do: ctx |> slice_lib(s) |> Path.dirname() |> Path.join("erts_vsn") |> File.read!() |> String.trim()

  # The Xcode the slices were built with: a new SDK rebuilds them.
  defp xcode(_ctx) do
    {out, 0} = System.cmd("xcodebuild", ["-version"])
    out
  end

  # ---------------------------------------------------------------------------------------------
  # Paths

  defp ios(ctx), do: Path.join(ctx.build, "ios")
  defp framework(ctx), do: Path.join(ios(ctx), "liberlang.xcframework")
  defp slice_lib(ctx, s), do: Path.join([ios(ctx), "slices", s.id, "liberlang.a"])
  defp openssl(ctx, s), do: Path.join(ios(ctx), "openssl-#{s.id}")
  defp libcrypto(ctx, s), do: Path.join(openssl(ctx, s), "lib/libcrypto.a")
  defp log_file(ctx, name), do: Path.join(ios(ctx), "#{name}.log")
  defp downloads(ctx), do: Path.join(ctx.build, "downloads")
  defp download_path(ctx, url), do: Path.join(downloads(ctx), Path.basename(url))
  defp otp_url, do: "https://github.com/erlang/otp/releases/download/OTP-#{@otp}/otp_src_#{@otp}.tar.gz"

  defp openssl_url,
    do: "https://github.com/openssl/openssl/releases/download/openssl-#{@openssl}/openssl-#{@openssl}.tar.gz"

  # ---------------------------------------------------------------------------------------------
  # Helpers (as in scripts/engine.exs)

  # Run `fun` unless the step's inputs match the last successful run and its outputs exist.
  defp once(ctx, name, inputs, outputs, fun) do
    fingerprint = :crypto.hash(:sha256, :erlang.term_to_binary(inputs)) |> Base.encode16(case: :lower)
    path = Path.join([ctx.build, ".stamps", Atom.to_string(name)])

    if not ctx.force and File.read(path) == {:ok, fingerprint} and Enum.all?(outputs, &File.exists?/1) do
      log("#{name}: up to date")
    else
      log("#{name}")
      started = System.monotonic_time(:second)
      File.rm(path)
      File.mkdir_p!(ios(ctx))
      fun.()
      File.mkdir_p!(Path.dirname(path))
      File.write!(path, fingerprint)
      log("#{name}: done in #{System.monotonic_time(:second) - started}s")
    end
  end

  defp stamp(ctx, name), do: File.read!(Path.join([ctx.build, ".stamps", Atom.to_string(name)]))

  defp download(ctx, url, sha256) do
    dest = download_path(ctx, url)

    if not (File.exists?(dest) and sha256(dest) == sha256) do
      File.mkdir_p!(downloads(ctx))
      log("downloading #{url}")
      partial = dest <> ".partial"
      Req.get!(url, into: File.stream!(partial), retry: :transient, max_retries: 3, receive_timeout: 120_000)
      got = sha256(partial)
      if got != sha256, do: fail("#{Path.basename(url)}: SHA-256 #{got}, expected #{sha256}")
      File.rename!(partial, dest)
    end

    dest
  end

  defp sha256(path) do
    path
    |> File.stream!(65_536)
    |> Enum.reduce(:crypto.hash_init(:sha256), &:crypto.hash_update(&2, &1))
    |> :crypto.hash_final()
    |> Base.encode16(case: :lower)
  end

  defp extract(tarball, dest) do
    File.rm_rf!(dest)
    File.mkdir_p!(dest)
    cmd!("tar", ["-xzf", tarball, "-C", dest, "--strip-components=1"])
  end

  # Run a command; its output goes to `log:` (appended) when given, and is returned.
  defp cmd!(bin, args, opts \\ []) do
    {log_path, opts} = Keyword.pop(opts, :log)
    {out, status} = System.cmd(bin, args, Keyword.merge([stderr_to_stdout: true], opts))
    if log_path, do: File.write!(log_path, out, [:append])

    if status != 0 do
      tail = out |> String.split("\n") |> Enum.take(-30) |> Enum.join("\n")

      fail(
        "#{Path.basename(bin)} #{Enum.join(args, " ")} exited with #{status}#{if log_path, do: " (full log: #{log_path})"}\n#{tail}"
      )
    end

    out
  end

  defp log(msg), do: IO.puts(:stderr, "#{IO.ANSI.bright()}==> #{msg}#{IO.ANSI.reset()}")

  defp fail(msg) do
    IO.puts(:stderr, "#{IO.ANSI.red()}ios: #{msg}#{IO.ANSI.reset()}")
    System.halt(1)
  end
end

Ios.run(System.argv())
