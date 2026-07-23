variable "namespace" {
  type    = string
  default = "fastmail-mcp"
}

# Sensitive values — in jaritanet these come from the secret backend
# (SOPS/Vault), passed in by the root module, never defaulted in git.
variable "postgres_password" {
  type      = string
  sensitive = true
}

variable "token_enc_key" {
  description = "32-byte base64 key for XChaCha20-Poly1305 token encryption"
  type        = string
  sensitive   = true
}

variable "session_secret" {
  type      = string
  sensitive = true
}

variable "hydra_system_secret" {
  type      = string
  sensitive = true
}

variable "github_client_id" {
  type      = string
  sensitive = true
}

variable "github_client_secret" {
  type      = string
  sensitive = true
}
