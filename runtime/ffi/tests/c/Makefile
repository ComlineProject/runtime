# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# The C harness: `vectors.c` against `comline.h`, linked with the staticlib,
# then the cdylib, then the staticlib again under ASan and UBSan, each
# replaying what `export_vectors` writes.
#
#   make -C runtime/ffi/tests/c          # build the libraries, export, run all three
#   make -C runtime/ffi/tests/c static   # one of them (also: dynamic, sanitize)
#
# It honours CARGO_TARGET_DIR. CC defaults to cc; CI runs it with gcc.

ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))../../../..)
TARGET_DIR ?= $(or $(CARGO_TARGET_DIR),$(ROOT)/target)
LIB_DIR := $(TARGET_DIR)/release
VECTORS ?= $(TARGET_DIR)/ffi-vectors
OUT := $(TARGET_DIR)/ffi-c
CARGO ?= cargo

CFLAGS ?= -std=c11 -Wall -Wextra -Werror -pedantic -O1 -g
CPPFLAGS += -I$(ROOT)/runtime/ffi/include
# What a Rust staticlib needs from the system on Linux
# (`cargo rustc -- --print native-static-libs`).
NATIVE_LIBS ?= -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc
SANITIZE ?= -fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer

SRC := $(ROOT)/runtime/ffi/tests/c/vectors.c
HEADER := $(ROOT)/runtime/ffi/include/comline.h

.PHONY: all check static dynamic sanitize libs export clean

all: check
check: static dynamic sanitize

libs:
	$(CARGO) build --manifest-path $(ROOT)/Cargo.toml -p comline-runtime-ffi --release

export:
	$(CARGO) run --manifest-path $(ROOT)/Cargo.toml -p comline-runtime-ffi --example export_vectors -- $(VECTORS)

$(OUT)/vectors-static: $(SRC) $(HEADER) libs
	@mkdir -p $(OUT)
	$(CC) $(CPPFLAGS) $(CFLAGS) $(SRC) -o $@ $(LIB_DIR)/libcomline_runtime_ffi.a $(NATIVE_LIBS)

$(OUT)/vectors-dynamic: $(SRC) $(HEADER) libs
	@mkdir -p $(OUT)
	$(CC) $(CPPFLAGS) $(CFLAGS) $(SRC) -o $@ -L$(LIB_DIR) -l:libcomline_runtime_ffi.so -Wl,-rpath,$(LIB_DIR)

$(OUT)/vectors-sanitize: $(SRC) $(HEADER) libs
	@mkdir -p $(OUT)
	$(CC) $(CPPFLAGS) $(CFLAGS) $(SANITIZE) $(SRC) -o $@ $(LIB_DIR)/libcomline_runtime_ffi.a $(NATIVE_LIBS)

static: $(OUT)/vectors-static export
	@echo "== staticlib"
	$(OUT)/vectors-static $(VECTORS)

dynamic: $(OUT)/vectors-dynamic export
	@echo "== cdylib"
	@# It must be the cdylib that answers, not a copy linked in.
	@ldd $(OUT)/vectors-dynamic | grep -q 'libcomline_runtime_ffi.so' || { echo "vectors-dynamic doesn't load the cdylib"; exit 1; }
	$(OUT)/vectors-dynamic $(VECTORS)

sanitize: $(OUT)/vectors-sanitize export
	@echo "== staticlib, ASan + UBSan"
	ASAN_OPTIONS=detect_leaks=1 $(OUT)/vectors-sanitize $(VECTORS)

clean:
	rm -rf $(OUT)
