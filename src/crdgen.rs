pub mod bitwarden_cli;
pub mod monitoring;
pub mod operator;

use crate::operator::schemas::BitwardenSecret;
use kube::CustomResourceExt;

fn main() {
    print!(
        "{}",
        serde_yaml::to_string(&BitwardenSecret::crd()).unwrap()
    )
}
