# SPDX-FileCopyrightText: 2026 The weft authors
# SPDX-License-Identifier: MIT
"""StrictDoc configuration: requirements in requirements/, traced to src/, module/ and guest/."""

from strictdoc.core.project_config import ProjectConfig


def create_config() -> ProjectConfig:
    return ProjectConfig(
        project_title="weft-sim requirements",
        project_features=[
            "REQUIREMENT_TO_SOURCE_TRACEABILITY",
            "TRACEABILITY_MATRIX_SCREEN",
        ],
        include_doc_paths=["requirements/**"],
        source_root_path=".",
        include_source_paths=["src/**", "module/**", "guest/**"],
        # Kernel build outputs next to the module source.
        exclude_source_paths=[
            "module/*.ko",
            "module/*.o",
            "module/*.mod",
            "module/*.mod.c",
            "module/.*",
            "module/Module.symvers",
            "module/modules.order",
        ],
    )
