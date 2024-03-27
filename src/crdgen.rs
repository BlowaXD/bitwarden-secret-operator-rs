// crdgen.rs
pub mod operator;

use kube::CustomResourceExt;
use crate::operator::schemas::BitwardenSecret;

fn main() {
    print!("{}", serde_yaml::to_string(&BitwardenSecret::crd()).unwrap())
}
