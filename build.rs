// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED
//
// Protobuf code generation for gRPC transport.
// Only runs when transport-grpc or transport-grpc-vector-compat features are enabled.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // DFE native transport proto
    #[cfg(feature = "transport-grpc")]
    {
        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .compile_protos(&["proto/dfe/transport/v1/dfe_transport.proto"], &["proto"])?;
    }

    // Vector wire protocol compat (vendored protos)
    #[cfg(feature = "transport-grpc-vector-compat")]
    {
        // Compile event.proto first (message types only, no services)
        prost_build::Config::new()
            .compile_protos(&["proto/vector/event.proto"], &["proto/vector"])?;

        // Compile vector.proto (service + request/response types)
        // extern_path tells prost the event types live in the sibling module
        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .extern_path(".event", "crate::transport::vector_compat::proto::event")
            .compile_protos(&["proto/vector/vector.proto"], &["proto/vector"])?;
    }

    Ok(())
}
