{
  description = "estante — cargo for shell. Nix-native, git-as-registry, Rust + Tatara-Lisp shell-package manager.";

  # substrate.rust.workspace dispatches over Cargo.gen.lock (the slim gen delta,
  # reconstructed to the full BuildSpec in pure Nix) — no crate2nix, no Cargo.nix.
  inputs.substrate.url = "github:pleme-io/substrate";

  outputs = { substrate, ... }: substrate.rust.workspace {
    src = ./.;
    member = "estante";
  };
}
