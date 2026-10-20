#!/usr/bin/env elixir

# Build the ampoule engine: ERTS compiled to wasm32-wasip1-threads (build/engine/beam.wasm) and
# the kit `ampoule pack` relinks it from when an app has NIFs (build/engine/kit).
#
#   elixir scripts/engine.exs [STEP...] [--build DIR] [--jobs N] [--force]
#
# Steps, run in this order (default: all of them):
# (downloads are pinned and SHA-256 checked)
#
#   sdk      download wasi-sdk (also what `ampoule pack` needs to build NIFs)
#   sources  download OTP and OpenSSL
#   openssl  build OpenSSL for wasm (libcrypto.a, headers)
#   aos     build libaos.a from engine/ampoule-os
#   erts     build ERTS from a fresh, patched OTP tree (engine/patches/erts-wasi.patch)
#   kit      assemble build/engine/{beam.wasm,kit}
#
# A step is skipped when its inputs are unchanged since it last succeeded (--force reruns it).
# ERTS is always built from a fresh tree: OTP's Makefiles do not track flags, and objects left
# from a previous configuration silently keep its feature macros.
#
# Needs a host Erlang/OTP of the same major as the engine on PATH (`mise install` provides it),
# plus make, perl, tar and patch.

Mix.install([{:req, "~> 0.7"}])

defmodule Engine do
  @root Path.expand("..", __DIR__)
  @target "wasm32-unknown-wasi"
  @triple "wasm32-wasip1-threads"
  @steps ~w(sdk sources openssl aos erts kit)

  @wasi_sdk "34.0"
  @wasi_sdk_sha256 %{
    "arm64-macos" => "9c59398106b417f8f14913380fdf0097a8cc0ff4af9eb3ce0065a859e88d49e9",
    "x86_64-macos" => "87d27fa8adc68dee59bfbf2e22a6d34ef717c34d6bf1d8af2a56fc929d9ce0eb",
    "arm64-linux" => "f7e243dff54d60bcc576e94d6166b69f410f2500ae4a9ceef34315be10e77971",
    "x86_64-linux" => "b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4"
  }
  @otp "28.5.0.4"
  @otp_sha256 "efb045f96ee56d274f6c1fe3a9612b45caa01d2f82e35fe4e0ac9b0c501f6e53"
  @openssl "3.5.9"
  @openssl_sha256 "603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a"

  # wasi-libc emulates these POSIX pieces behind feature macros; ERTS, OpenSSL and NIFs all use them.
  @emulated ~w(-D_WASI_EMULATED_SIGNAL -D_WASI_EMULATED_PROCESS_CLOCKS -D_WASI_EMULATED_MMAN -D_WASI_EMULATED_GETPID)
  @emulated_libs ~w(-lwasi-emulated-mman -lwasi-emulated-signal -lwasi-emulated-process-clocks -lwasi-emulated-getpid)

  # Link-time optimisation of the emulator: about 11% off interpreter-bound work (a reduce over 5M
  # elements, 137 -> 122 ms) for 9.7% more beam.wasm. -flto goes to make, not configure's CFLAGS:
  # configure's probe for a static OpenSSL links a -shared test program, which fails as bitcode.
  # The kit's objects are then LLVM bitcode, so relinking with NIFs needs this exact LLVM (kit.json).
  @lto_make ~w(ARCHCFLAGS=-flto)

  @openssl_options ~w(linux-generic32 no-asm no-shared no-dso no-engine no-afalgeng no-ui-console no-tests no-apps no-docs
                      no-module no-sock no-secure-memory no-async no-thread-pool no-default-thread-pool)

  @nif_headers ~w(erl_nif.h erl_nif_api_funcs.h erl_drv_nif.h erl_driver.h erl_fixed_size_int_types.h)

  def run(argv) do
    Logger.configure(level: :info)
    {opts, steps} = OptionParser.parse!(argv, strict: [build: :string, jobs: :integer, force: :boolean])
    steps = if steps == [], do: @steps, else: steps
    for s <- steps, s not in @steps, do: fail("unknown step #{s} (steps: #{Enum.join(@steps, ", ")})")

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

  defp step(:sdk, ctx) do
    host = host()

    url =
      "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-#{major(@wasi_sdk)}/wasi-sdk-#{@wasi_sdk}-#{host}.tar.gz"

    once(ctx, :sdk, [@wasi_sdk, host], [clang(ctx)], fn ->
      extract(download(ctx, url, Map.fetch!(@wasi_sdk_sha256, host)), sdk(ctx))
    end)
  end

  defp step(:sources, ctx) do
    tarballs = [download_path(ctx, otp_url()), download_path(ctx, openssl_url())]

    once(ctx, :sources, [@otp, @openssl], tarballs, fn ->
      download(ctx, otp_url(), @otp_sha256)
      download(ctx, openssl_url(), @openssl_sha256)
    end)
  end

  defp step(:openssl, ctx) do
    once(
      ctx,
      :openssl,
      [@openssl, @openssl_options, @emulated, @wasi_sdk],
      [Path.join(openssl(ctx), "lib/libcrypto.a")],
      fn ->
        src = Path.join(ctx.build, "openssl-src")
        extract(download_path(ctx, openssl_url()), src)

        env = [
          {"CC", "#{clang(ctx)} --target=#{@triple}"},
          {"AR", tool(ctx, "llvm-ar")},
          {"RANLIB", tool(ctx, "llvm-ranlib")}
        ]

        cflags = Enum.join(["-O2", "-pthread" | @emulated], " ")

        cmd!(
          "perl",
          ["Configure" | @openssl_options] ++ ["--prefix=#{openssl(ctx)}", "--libdir=lib", "CFLAGS=#{cflags}"],
          cd: src,
          env: env
        )

        cmd!("make", ["-j#{ctx.jobs}", "build_libs"], cd: src)
        cmd!("make", ["install_dev"], cd: src)
      end
    )
  end

  defp step(:aos, ctx) do
    srcs = Path.wildcard(Path.join(@root, "engine/ampoule-os/**/*.{c,h}"))
    lib = Path.join(ctx.build, "aos/libaos.a")

    once(ctx, :aos, [@wasi_sdk | Enum.map(srcs, &File.read!/1)], [lib], fn ->
      obj = Path.join(ctx.build, "aos/aos.o")
      File.mkdir_p!(Path.dirname(obj))

      cmd!(clang(ctx), [
        "--target=#{@triple}",
        "-pthread",
        "-O2",
        "-c",
        Path.join(@root, "engine/ampoule-os/src/aos.c"),
        "-o",
        obj
      ])

      File.rm(lib)
      cmd!(tool(ctx, "llvm-ar"), ["rcs", lib, obj])
    end)
  end

  defp step(:erts, ctx) do
    check_host_otp!()
    patch = Path.join(@root, "engine/patches/erts-wasi.patch")
    headers = Path.wildcard(Path.join(@root, "engine/ampoule-os/include/**/*.h"))
    conf = xcomp_conf(ctx)
    inputs = [@otp, File.read!(patch), conf, @lto_make, stamp(ctx, :openssl), stamp(ctx, :aos) | Enum.map(headers, &File.read!/1)]

    once(ctx, :erts, inputs, [beam(ctx)], fn ->
      otp = otp(ctx)
      File.rm_rf!(otp)
      extract(download_path(ctx, otp_url()), otp)
      # Not `git apply`: inside this repository it would resolve paths against the repository root.
      cmd!("patch", ["-p1", "--forward", "--batch", "-i", patch], cd: otp)
      conf_path = Path.join(ctx.build, "erl-xcomp-#{@target}.conf")
      File.write!(conf_path, conf)
      log("configure (log: #{Path.join(ctx.build, "configure.log")})")

      cmd!(Path.join(otp, "otp_build"), ["configure", "--xcomp-conf=#{conf_path}"],
        cd: otp,
        env: [{"ERL_TOP", otp}, {"TARGET", nil}],
        log: Path.join(ctx.build, "configure.log")
      )

      # Only the emulator: its static NIFs (crypto, asn1) are built on demand, and the rest of
      # erts (run_erl, heart, epmd) are host programs that cannot run in a wasm guest anyway.
      for dir <- ["erts/lib_src", "erts/emulator"] do
        log("make #{dir}")

        cmd!("make", ["-j#{ctx.jobs}", "opt", "FLAVOR=emu" | @lto_make],
          cd: Path.join(otp, dir),
          env: otp_env(ctx),
          log: Path.join(ctx.build, "erts.log")
        )
      end
    end)
  end

  defp step(:kit, ctx) do
    out = Path.join(ctx.build, "engine")

    once(
      ctx,
      :kit,
      [stamp(ctx, :erts)],
      [Path.join(out, "beam.wasm"), Path.join(out, "erts_vsn"), Path.join(out, "kit/kit.json")],
      fn ->
        File.rm_rf!(out)
        File.mkdir_p!(out)
        File.cp!(beam(ctx), Path.join(out, "beam.wasm"))
        File.write!(Path.join(out, "erts_vsn"), erts_vsn(ctx) <> "\n")
        build_kit(ctx, Path.join(out, "kit"))
      end
    )
  end

  # ---------------------------------------------------------------------------------------------
  # The kit: everything needed to relink beam.wasm with extra static NIFs.
  #
  #   kit.json      ERTS version, link flags, and the order of objects/archives
  #   obj/          the emulator's objects (driver_tab.o excluded: pack regenerates it)
  #   lib/          archives: OTP's static NIFs, OpenSSL, zlib, ethread, libaos, ...
  #   driver_tab.i  the preprocessed driver/NIF table, so new entries compile without ERTS headers
  #   include/      erl_nif.h and friends, for compiling NIF sources
  #   ampoule-os/  the header force-included into every compile, and its compat headers

  defp build_kit(ctx, out) do
    Enum.each(~w(obj lib include ampoule-os), &File.mkdir_p!(Path.join(out, &1)))
    {link_flags, objects, archives} = link_line(ctx)

    objects =
      for {obj, i} <- Enum.with_index(objects) do
        name = "#{String.pad_leading(Integer.to_string(i), 3, "0")}-#{Path.basename(obj)}"
        File.cp!(obj, Path.join([out, "obj", name]))
        "obj/#{name}"
      end

    archives =
      for a <- archives do
        File.cp!(a, Path.join([out, "lib", Path.basename(a)]))
        "lib/#{Path.basename(a)}"
      end

    preprocess_driver_tab(ctx, Path.join(out, "driver_tab.i"))
    erts = Path.join(otp(ctx), "erts")

    for h <- @nif_headers do
      src = Enum.find([Path.join([erts, "emulator/beam", h]), Path.join([erts, "include", h])], &File.exists?/1)
      File.cp!(src, Path.join([out, "include", h]))
    end

    File.cp!(
      Path.join([erts, "include", @target, "erl_int_sizes_config.h"]),
      Path.join([out, "include", "erl_int_sizes_config.h"])
    )

    File.cp_r!(Path.join(@root, "engine/ampoule-os/include"), Path.join(out, "ampoule-os"))

    kit = %{
      format: 1,
      erts_vsn: erts_vsn(ctx),
      # The objects are LLVM bitcode (@lto_make): only this LLVM can link them.
      llvm_version: llvm_version(ctx),
      link_flags: link_flags,
      objects: objects,
      archives: archives,
      system_libs: ~w(-ldl -lm) ++ @emulated_libs ++ ~w(-lpthread)
    }

    File.write!(Path.join(out, "kit.json"), JSON.encode!(kit))

    size =
      out
      |> Path.join("**/*")
      |> Path.wildcard()
      |> Enum.filter(&File.regular?/1)
      |> Enum.map(&File.stat!(&1).size)
      |> Enum.sum()

    log(
      "kit: ERTS #{kit.erts_vsn}, #{length(objects)} objects, #{length(archives)} archives, #{div(size, 1_048_576)} MiB"
    )
  end

  # The emulator's final link, from a verbose relink: flags, objects and archives, in order.
  defp link_line(ctx) do
    emu = Path.join(otp(ctx), "erts/emulator")
    bin = Path.join([otp(ctx), "bin", @target, "beam.emu"])
    File.rm(bin)
    out = cmd!("make", ["-f", "#{@target}/Makefile", "TYPE=opt", "FLAVOR=emu", "V=1", bin | @lto_make], cd: emu, env: otp_env(ctx))

    args =
      out
      |> String.split("\n")
      |> Enum.drop_while(&(not Regex.match?(~r/-o \S*beam\.emu/, &1)))
      |> Enum.reduce_while([], fn line, acc ->
        if String.ends_with?(line, "\\"), do: {:cont, [line | acc]}, else: {:halt, [line | acc]}
      end)
      |> Enum.reverse()
      |> Enum.map_join(" ", &String.trim_trailing(&1, "\\"))
      |> String.split()
      |> Enum.drop(1)

    libdir = Path.join(otp(ctx), "erts/lib/internal/#{@target}")
    {objects, rest} = Enum.split_with(args, &String.ends_with?(&1, ".o"))
    objects = objects |> Enum.reject(&String.ends_with?(&1, "driver_tab.o")) |> Enum.map(&Path.expand(&1, emu))

    {archives, flags} =
      rest
      |> drop_output()
      |> Enum.flat_map(fn
        "-lethread" -> [Path.join(libdir, "libethread.a")]
        "-lerts_internal_r" -> [Path.join(libdir, "liberts_internal_r.a")]
        "-L" <> _ -> []
        "--target=" <> _ -> []
        a -> [a]
      end)
      |> Enum.split_with(&String.ends_with?(&1, ".a"))

    {Enum.reject(flags, &String.starts_with?(&1, "-l")), objects, Enum.map(archives, &Path.expand(&1, emu))}
  end

  defp preprocess_driver_tab(ctx, dest) do
    emu = Path.join(otp(ctx), "erts/emulator")
    obj = "obj/#{@target}/opt/emu/driver_tab.o"
    File.rm(Path.join(emu, obj))
    out = cmd!("make", ["-f", "#{@target}/Makefile", "TYPE=opt", "FLAVOR=emu", "V=1", obj], cd: emu, env: otp_env(ctx))
    [cmd] = out |> String.split("\n") |> Enum.filter(&String.contains?(&1, "driver_tab.c -o"))
    [cc | args] = String.split(cmd)
    args = args |> Enum.reject(&(&1 == "-c")) |> drop_output()
    cmd!(cc, args ++ ["-E", "-P", "-o", dest], cd: emu)
  end

  defp drop_output(["-o", _ | rest]), do: rest
  defp drop_output([a | rest]), do: [a | drop_output(rest)]
  defp drop_output([]), do: []

  # ---------------------------------------------------------------------------------------------
  # OTP cross-compilation settings

  defp xcomp_conf(ctx) do
    sdk = sdk(ctx)
    aos = Path.join(@root, "engine/ampoule-os/include")

    cflags =
      Enum.join(
        ["-O3", "-pthread", "-D_GNU_SOURCE"] ++
          @emulated ++ ["-isystem #{aos}/compat", "-include #{aos}/ampoule_os.h"],
        " "
      )

    libs =
      Enum.join(
        [Path.join(ctx.build, "aos/libaos.a"), Path.join(openssl(ctx), "lib/libcrypto.a") | @emulated_libs],
        " "
      )

    configure_flags =
      ~w(--disable-jit --without-termcap --disable-dynamic-ssl-lib --disable-esock --disable-kernel-poll --disable-pie
         --enable-static-nifs --enable-static-drivers --without-dynamic-trace --without-wx --without-odbc --without-javac
         ac_cv_func_mallopt=no)

    """
    ## Generated by scripts/engine.exs: cross-compile ERTS to #{@triple} with wasi-sdk #{@wasi_sdk}.
    erl_xcomp_build=guess
    erl_xcomp_host=#{@target}
    erl_xcomp_sysroot=#{sdk}/share/wasi-sysroot
    erl_xcomp_configure_flags="#{Enum.join(configure_flags, " ")} --with-ssl=#{openssl(ctx)}"
    CC="#{clang(ctx)} --target=#{@triple}"
    CXX="#{sdk}/bin/clang++ --target=#{@triple}"
    LD="#{clang(ctx)} --target=#{@triple}"
    AR=#{tool(ctx, "llvm-ar")}
    RANLIB=#{tool(ctx, "llvm-ranlib")}
    CFLAGS="#{cflags}"
    CXXFLAGS="-O3 -pthread"
    LDFLAGS="-pthread -Wl,--lto-O3 -Wl,--import-memory,--max-memory=4294967296 -Wl,-z,stack-size=4194304"
    LIBS="#{libs}"
    erl_xcomp_bigendian=no
    erl_xcomp_double_middle_endian=no
    erl_xcomp_clock_gettime_cpu_time=no
    erl_xcomp_getaddrinfo=no
    erl_xcomp_gethrvtime_procfs_ioctl=no
    erl_xcomp_dlsym_brk_wrappers=no
    erl_xcomp_kqueue=no
    erl_xcomp_linux_clock_gettime_correction=no
    erl_xcomp_linux_nptl=no
    erl_xcomp_linux_usable_sigaltstack=no
    erl_xcomp_linux_usable_sigusrx=no
    erl_xcomp_poll=yes
    erl_xcomp_putenv_copy=no
    erl_xcomp_reliable_fpe=no
    erl_xcomp_posix_memalign=yes
    erl_xcomp_after_morecore_hook=no
    erl_xcomp_code_model_small=no
    """
  end

  # For make. Not for configure: otp_build takes TARGET as the *build* system.
  defp otp_env(ctx), do: [{"ERL_TOP", otp(ctx)}, {"TARGET", @target}]

  defp check_host_otp! do
    want = @otp |> String.split(".") |> hd()
    have = System.otp_release()

    if have != want,
      do:
        fail(
          "building ERTS #{@otp} needs a host Erlang/OTP #{want} on PATH, found #{have} (run through `mise exec --`)"
        )
  end

  defp erts_vsn(ctx) do
    ~r/^VSN = (.+)$/m
    |> Regex.run(File.read!(Path.join(otp(ctx), "erts/vsn.mk")), capture: :all_but_first)
    |> hd()
    |> String.trim()
  end

  # ---------------------------------------------------------------------------------------------
  # Paths

  defp sdk(ctx), do: Path.join(ctx.build, "wasi-sdk")

  # wasi-sdk's VERSION file names its LLVM: "llvm-version: 23.1.0".
  defp llvm_version(ctx) do
    sdk(ctx)
    |> Path.join("VERSION")
    |> File.read!()
    |> then(&Regex.run(~r/^llvm-version: (\S+)$/m, &1, capture: :all_but_first))
    |> case do
      [v] -> v
      _ -> fail("no llvm-version in #{Path.join(sdk(ctx), "VERSION")}")
    end
  end
  defp clang(ctx), do: Path.join(sdk(ctx), "bin/clang")
  defp tool(ctx, name), do: Path.join(sdk(ctx), "bin/#{name}")
  defp openssl(ctx), do: Path.join(ctx.build, "openssl")
  defp otp(ctx), do: Path.join(ctx.build, "otp")
  defp beam(ctx), do: Path.join([otp(ctx), "bin", @target, "beam.smp"])
  defp downloads(ctx), do: Path.join(ctx.build, "downloads")
  defp download_path(ctx, url), do: Path.join(downloads(ctx), Path.basename(url))
  defp otp_url, do: "https://github.com/erlang/otp/releases/download/OTP-#{@otp}/otp_src_#{@otp}.tar.gz"

  defp openssl_url,
    do: "https://github.com/openssl/openssl/releases/download/openssl-#{@openssl}/openssl-#{@openssl}.tar.gz"

  defp major(v), do: v |> String.split(".") |> hd()

  defp host do
    arch =
      case :erlang.system_info(:system_architecture) |> to_string() |> String.split("-") |> hd() do
        a when a in ["aarch64", "arm64"] -> "arm64"
        "x86_64" -> "x86_64"
        other -> fail("no wasi-sdk build for #{other}")
      end

    os = if match?({:unix, :darwin}, :os.type()), do: "macos", else: "linux"
    "#{arch}-#{os}"
  end

  # ---------------------------------------------------------------------------------------------
  # Helpers

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
    IO.puts(:stderr, "#{IO.ANSI.red()}engine: #{msg}#{IO.ANSI.reset()}")
    System.halt(1)
  end
end

Engine.run(System.argv())
