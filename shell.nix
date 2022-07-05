{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    cargo
    pkgconfig
    rust-analyzer
    rustc
    rustfmt
    llvmPackages.clang
  ];
  buildInputs = with pkgs; [
    leptonica
    tesseract5
  ];
  LIBCLANG_PATH = pkgs.lib.makeLibraryPath [ pkgs.libclang.lib ];
}
