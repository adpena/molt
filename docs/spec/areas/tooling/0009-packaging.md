# Molt Packaging & Distribution

The [release authority](../../../../packaging/PACKAGING.md) owns candidate
assembly, independent build comparisons, provenance, trust and release delivery.
The [public stable contract](../../PUBLIC_CONTRACT_V1.md) owns advertised surfaces
and their release acceptance obligations.

Use [toolchain custody](0001-toolchains.md) for executable and locked-input
admission, the [extension ABI contract](../compat/contracts/libmolt_extension_abi_contract.md)
for source-built native extensions, and the
[capability contract](../../../CAPABILITIES.md) for host permissions.

Reproducibility, installed execution, signatures and semantic release acceptance
require evidence for the declared source, toolchain and product coordinates.
Configured workflow steps, matching dependency locks and header coverage alone
do not establish those results. Native OS signing/notarization, package-manager
availability and registry publication retain their delivery obligations in the
release authority.
