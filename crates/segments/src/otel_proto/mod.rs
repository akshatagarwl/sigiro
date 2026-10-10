pub mod tonic {
    pub mod common {
        pub mod v1 {
            #![allow(dead_code, clippy::all)]
            include!("tonic/opentelemetry.proto.common.v1.rs");
        }
    }

    pub mod resource {
        pub mod v1 {
            #![allow(dead_code, clippy::all)]
            include!("tonic/opentelemetry.proto.resource.v1.rs");
        }
    }

    pub mod trace {
        pub mod v1 {
            #![allow(dead_code, clippy::all)]
            include!("tonic/opentelemetry.proto.trace.v1.rs");
        }
    }

    pub mod logs {
        pub mod v1 {
            #![allow(dead_code, clippy::all)]
            include!("tonic/opentelemetry.proto.logs.v1.rs");
        }
    }

    pub mod metrics {
        pub mod v1 {
            #![allow(dead_code, clippy::all)]
            include!("tonic/opentelemetry.proto.metrics.v1.rs");
        }
    }

    pub mod collector {
        pub mod trace {
            pub mod v1 {
                #![allow(dead_code, clippy::all)]
                include!("tonic/opentelemetry.proto.collector.trace.v1.rs");
            }
        }

        pub mod logs {
            pub mod v1 {
                #![allow(dead_code, clippy::all)]
                include!("tonic/opentelemetry.proto.collector.logs.v1.rs");
            }
        }

        pub mod metrics {
            pub mod v1 {
                #![allow(dead_code, clippy::all)]
                include!("tonic/opentelemetry.proto.collector.metrics.v1.rs");
            }
        }
    }
}
