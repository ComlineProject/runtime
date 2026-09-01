# comline-runtime

The Comline RPC/IPC runtime — the surface-4 contract (`Dispatch`, `WireFormat`,
`Envelope`, `Handshake`, …), framing, transports, and the `Client` / `Server`
call path. `no_std`-first; `alloc` and `std` are additive features.

Generated protocol code (`comline-codegen-<lang>` output) links this crate.

## License

**Mozilla Public License 2.0** ([LICENSE](LICENSE) or
<https://www.mozilla.org/MPL/2.0/>).

Weak, file-level copyleft: modifications to this crate's own files stay open
when distributed, but linking it into a proprietary application is explicitly
fine (MPL §3.3). It is the one piece of Comline meant to ship *inside* your
program, which is why it is not GPL like the toolchain. Contributions are
MPL-2.0. See [`design/licensing.md`](https://github.com/ComlineProject/docs)
for the rationale and the per-repo split.
