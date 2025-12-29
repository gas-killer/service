# =============================================================================
# Terraform Backend Configuration
# =============================================================================
#
# For production use, uncomment and configure the S3 backend below.
# This enables state sharing and locking across team members.
#
# Prerequisites:
# 1. Create an S3 bucket for state storage
# 2. Create a DynamoDB table for state locking (optional but recommended)
#
# Example setup commands:
#   aws s3 mb s3://your-terraform-state-bucket --region us-east-1
#   aws dynamodb create-table \
#     --table-name terraform-locks \
#     --attribute-definitions AttributeName=LockID,AttributeType=S \
#     --key-schema AttributeName=LockID,KeyType=HASH \
#     --billing-mode PAY_PER_REQUEST
# =============================================================================

# terraform {
#   backend "s3" {
#     bucket         = "your-terraform-state-bucket"
#     key            = "gas-killer/dev/terraform.tfstate"
#     region         = "us-east-1"
#     encrypt        = true
#     dynamodb_table = "terraform-locks"  # Optional: for state locking
#   }
# }
