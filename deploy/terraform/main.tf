# Deploys the stack: namespace + the sensitive Secret (managed by TF so it can
# be sourced from the secret backend), then applies the workload manifests from
# ../k8s verbatim. The manifests reference the Secret by name.

resource "kubernetes_namespace" "ns" {
  metadata {
    name = var.namespace
  }
}

resource "kubernetes_secret" "secrets" {
  metadata {
    name      = "fastmail-mcp-secrets"
    namespace = kubernetes_namespace.ns.metadata[0].name
  }
  data = {
    postgres-user        = "fastmail"
    postgres-password    = var.postgres_password
    database-url         = "postgres://fastmail:${var.postgres_password}@postgres:5432/fastmail_mcp"
    hydra-dsn            = "postgres://fastmail:${var.postgres_password}@postgres:5432/hydra?sslmode=disable"
    hydra-system-secret  = var.hydra_system_secret
    token-enc-key        = var.token_enc_key
    session-secret       = var.session_secret
    github-client-id     = var.github_client_id
    github-client-secret = var.github_client_secret
  }
}

# Apply every workload manifest except the ones TF owns itself
# (namespace, the secret template).
locals {
  manifest_files = [
    for f in fileset("${path.module}/../k8s", "*.yaml") :
    "${path.module}/../k8s/${f}"
    if !contains(["namespace.yaml", "secrets.example.yaml", "kustomization.yaml"], f)
  ]
}

data "kubectl_file_documents" "workloads" {
  content = join("\n---\n", [for f in local.manifest_files : file(f)])
}

resource "kubectl_manifest" "workloads" {
  for_each  = data.kubectl_file_documents.workloads.manifests
  yaml_body = each.value

  depends_on = [
    kubernetes_namespace.ns,
    kubernetes_secret.secrets,
  ]
}
