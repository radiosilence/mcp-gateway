# Provider *configuration* (kubeconfig / cluster auth) and the backend are
# supplied by jaritanet's root module — this is a child module. It only
# declares what it needs.
terraform {
  required_version = ">= 1.5"
  required_providers {
    kubernetes = {
      source  = "hashicorp/kubernetes"
      version = ">= 2.30"
    }
    kubectl = {
      source  = "gavinbunney/kubectl"
      version = ">= 1.14"
    }
  }
}
