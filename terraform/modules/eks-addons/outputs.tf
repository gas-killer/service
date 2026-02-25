output "ebs_csi_driver_role_arn" {
  description = "ARN of the EBS CSI driver IAM role"
  value       = aws_iam_role.ebs_csi_driver.arn
}

output "alb_controller_role_arn" {
  description = "ARN of the ALB controller IAM role"
  value       = aws_iam_role.alb_controller.arn
}

output "storage_class_name" {
  description = "Name of the default storage class"
  value       = kubernetes_storage_class.gp3.metadata[0].name
}

output "ready" {
  description = "Indicates that all add-ons are ready"
  value       = true

  depends_on = [
    aws_eks_addon.ebs_csi_driver,
    helm_release.alb_controller,
    kubernetes_storage_class.gp3,
  ]
}
