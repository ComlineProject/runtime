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
# It honours CARGO_TARGET_DIR. CC defaults to cc; CI runs it with gcc on
# Linux and clang on macOS and Windows. On Windows (an MSVC Rust toolchain,
# clang, a POSIX shell for make) the libraries are `comline_runtime_ffi.lib`
# and `comline_runtime_ffi.dll` and there is no sanitizer run; on macOS the
# cdylib is a `.dylib` and LeakSanitizer isn't there to ask for.

ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))../../../..)
TARGET_DIR ?= $(or $(CARGO_TARGET_DIR),$(ROOT)/target)
LIB_DIR := $(TARGET_DIR)/release
VECTORS ?= $(TARGET_DIR)/ffi-vectors
OUT := $(TARGET_DIR)/ffi-c
CARGO ?= cargo

CFLAGS ?= -std=c11 -Wall -Wextra -Werror -pedantic -O1 -g
CPPFLAGS += -I$(ROOT)/runtime/ffi/include
SANITIZE ?= -fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer

# What a Rust staticlib needs from the system
# (`cargo rustc -- --print native-static-libs`), and how each platform names
# and finds the libraries.
ifeq ($(OS),Windows_NT)
PLATFORM := windows
else
PLATFORM := $(shell uname -s)
endif

ifeq ($(PLATFORM),windows)
EXE := .exe
STATICLIB := comline_runtime_ffi.lib
DYLIB := comline_runtime_ffi.dll
# `fopen` is deprecated by the MSVC headers; the DLL CRT, as Rust links.
CPPFLAGS += -D_CRT_SECURE_NO_WARNINGS
CFLAGS += -fms-runtime-lib=dll
NATIVE_LIBS ?= -lkernel32 -ladvapi32 -lntdll -luserenv -lws2_32 -ldbghelp -lbcrypt
# The import library; the DLL is copied next to the program, where Windows
# looks first.
DYNAMIC_LINK = $(LIB_DIR)/$(DYLIB).lib
DYNAMIC_DEPS = cp $(LIB_DIR)/$(DYLIB) $(OUT)/
DYNAMIC_CHECK = grep -qa '$(DYLIB)' $(OUT)/vectors-dynamic$(EXE)
CHECKS := static dynamic
else ifeq ($(PLATFORM),Darwin)
EXE :=
STATICLIB := libcomline_runtime_ffi.a
DYLIB := libcomline_runtime_ffi.dylib
NATIVE_LIBS ?= -lSystem -lc -lm
DYNAMIC_LINK = -L$(LIB_DIR) -lcomline_runtime_ffi -Wl,-rpath,$(LIB_DIR)
DYNAMIC_DEPS = true
DYNAMIC_CHECK = otool -L $(OUT)/vectors-dynamic | grep -q '$(DYLIB)'
ASAN_OPTIONS_RUN :=
CHECKS := static dynamic sanitize
else
EXE :=
STATICLIB := libcomline_runtime_ffi.a
DYLIB := libcomline_runtime_ffi.so
NATIVE_LIBS ?= -lgcc_s -lutil -lrt -lpthread -lm -ldl -lc
DYNAMIC_LINK = -L$(LIB_DIR) -l:$(DYLIB) -Wl,-rpath,$(LIB_DIR)
DYNAMIC_DEPS = true
DYNAMIC_CHECK = ldd $(OUT)/vectors-dynamic | grep -q '$(DYLIB)'
ASAN_OPTIONS_RUN := ASAN_OPTIONS=detect_leaks=1
CHECKS := static dynamic sanitize
endif

SRC := $(ROOT)/runtime/ffi/tests/c/vectors.c
HEADER := $(ROOT)/runtime/ffi/include/comline.h

.PHONY: all check static dynamic sanitize libs export clean

all: check
check: $(CHECKS)

libs:
	$(CARGO) build --manifest-path $(ROOT)/Cargo.toml -p comline-runtime-ffi --release

export:
	$(CARGO) run --manifest-path $(ROOT)/Cargo.toml -p comline-runtime-ffi --example export_vectors -- $(VECTORS)

$(OUT)/vectors-static: $(SRC) $(HEADER) libs
	@mkdir -p $(OUT)
	$(CC) $(CPPFLAGS) $(CFLAGS) $(SRC) -o $@$(EXE) $(LIB_DIR)/$(STATICLIB) $(NATIVE_LIBS)

$(OUT)/vectors-dynamic: $(SRC) $(HEADER) libs
	@mkdir -p $(OUT)
	$(CC) $(CPPFLAGS) $(CFLAGS) $(SRC) -o $@$(EXE) $(DYNAMIC_LINK)
	$(DYNAMIC_DEPS)

$(OUT)/vectors-sanitize: $(SRC) $(HEADER) libs
	@mkdir -p $(OUT)
	$(CC) $(CPPFLAGS) $(CFLAGS) $(SANITIZE) $(SRC) -o $@$(EXE) $(LIB_DIR)/$(STATICLIB) $(NATIVE_LIBS)

static: $(OUT)/vectors-static export
	@echo "== staticlib"
	$(OUT)/vectors-static$(EXE) $(VECTORS)

dynamic: $(OUT)/vectors-dynamic export
	@echo "== cdylib"
	@# It must be the cdylib that answers, not a copy linked in.
	@$(DYNAMIC_CHECK) || { echo "vectors-dynamic doesn't load the cdylib"; exit 1; }
	$(OUT)/vectors-dynamic$(EXE) $(VECTORS)

sanitize: $(OUT)/vectors-sanitize export
	@echo "== staticlib, ASan + UBSan"
	$(ASAN_OPTIONS_RUN) $(OUT)/vectors-sanitize$(EXE) $(VECTORS)

clean:
	rm -rf $(OUT)
