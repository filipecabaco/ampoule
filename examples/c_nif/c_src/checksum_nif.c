/* CRC-32 (IEEE 802.3, as zlib and Ethernet compute it) and FNV-1a, over a binary.
 *
 * Plain C NIF code: nothing in it knows about wasm. `ampoule release` compiles it for wasm and
 * links it into the engine. Long inputs run on a dirty CPU scheduler so they do not hold up the
 * VM's normal schedulers. */
#include <stdint.h>

#include "erl_nif.h"

/* Inputs above this many bytes go to a dirty scheduler. */
#define DIRTY_THRESHOLD (64 * 1024)

static uint32_t crc_table[256];

static void crc_init(void)
{
    for (uint32_t i = 0; i < 256; i++) {
        uint32_t c = i;
        for (int k = 0; k < 8; k++)
            c = (c & 1) ? 0xEDB88320u ^ (c >> 1) : c >> 1;
        crc_table[i] = c;
    }
}

static uint32_t crc32(const unsigned char *p, size_t n)
{
    uint32_t c = 0xFFFFFFFFu;
    for (size_t i = 0; i < n; i++)
        c = crc_table[(c ^ p[i]) & 0xFF] ^ (c >> 8);
    return c ^ 0xFFFFFFFFu;
}

static uint64_t fnv1a(const unsigned char *p, size_t n)
{
    uint64_t h = 0xcbf29ce484222325ull;
    for (size_t i = 0; i < n; i++) {
        h ^= p[i];
        h *= 0x100000001b3ull;
    }
    return h;
}

static ERL_NIF_TERM crc32_dirty(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[])
{
    ErlNifBinary bin;
    if (argc != 1 || !enif_inspect_binary(env, argv[0], &bin))
        return enif_make_badarg(env);
    return enif_make_uint(env, crc32(bin.data, bin.size));
}

static ERL_NIF_TERM crc32_nif(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[])
{
    ErlNifBinary bin;
    if (argc != 1 || !enif_inspect_binary(env, argv[0], &bin))
        return enif_make_badarg(env);
    if (bin.size > DIRTY_THRESHOLD)
        return enif_schedule_nif(env, "crc32_dirty", ERL_NIF_DIRTY_JOB_CPU_BOUND, crc32_dirty, argc, argv);
    return enif_make_uint(env, crc32(bin.data, bin.size));
}

static ERL_NIF_TERM fnv1a_nif(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[])
{
    ErlNifBinary bin;
    if (argc != 1 || !enif_inspect_binary(env, argv[0], &bin))
        return enif_make_badarg(env);
    return enif_make_uint64(env, fnv1a(bin.data, bin.size));
}

static int load(ErlNifEnv *env, void **priv, ERL_NIF_TERM info)
{
    (void)env;
    (void)priv;
    (void)info;
    crc_init();
    return 0;
}

static ErlNifFunc funcs[] = {
    {"crc32", 1, crc32_nif, 0},
    {"fnv1a", 1, fnv1a_nif, 0},
};

ERL_NIF_INIT(Elixir.Checksum, funcs, load, NULL, NULL, NULL)
