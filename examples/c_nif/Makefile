# Builds priv/checksum_nif.so from c_src/. elixir_make runs this with MIX_APP_PATH and
# ERTS_INCLUDE_DIR set; `ampoule release` runs it again with CC pointing at its wasm compiler,
# which turns the same commands into a wasm archive.
PRIV = $(MIX_APP_PATH)/priv
NIF = $(PRIV)/checksum_nif.so

CFLAGS += -O2 -std=c11 -Wall -Wextra -fPIC -I"$(ERTS_INCLUDE_DIR)"
ifeq ($(shell uname -s),Darwin)
	LDFLAGS += -dynamiclib -undefined dynamic_lookup
else
	LDFLAGS += -shared
endif

all: $(NIF)

$(NIF): c_src/checksum_nif.c
	@mkdir -p $(PRIV)
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<

clean:
	rm -f $(NIF)

.PHONY: all clean
