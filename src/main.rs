use milkpan::{
    BucketBuilder,
    cli::{BucketCommands, BucketHandle, resolve_home},
    provider::{github::GitHub, local::Local},
};
use panned::cli::{GenericCommands, Runner};

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let handle = BucketHandle::new(resolve_home(&args), |home| {
        BucketBuilder::new(home)
            .provider(GitHub::from_env())
            .provider(Local)
            .build()
    });
    Runner::new("milkpan", env!("CARGO_PKG_VERSION"))
        .with(GenericCommands::new(handle.clone(), handle.clone()).with_slots(handle.clone()))
        .with(BucketCommands::new(handle))
        .main()
}
