{ pkgs ? import <unstable> {} }: with pkgs;

mkShell {
  buildInputs = [
      rustup
  ];
}
