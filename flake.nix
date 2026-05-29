{
  description = "Fumi (文) — GPU-rendered multi-protocol chat client";

  inputs.substrate.url = "github:pleme-io/substrate";

  outputs = { substrate, ... }: substrate.rust.tool { src = ./.; };
}
