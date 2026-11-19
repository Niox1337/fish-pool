from dataclasses import dataclass


@dataclass
class PlatformMetadata:
    name: str
    """Adapter type, for example qq_official."""
    description: str = ""
    id: str = ""
    """Adapter instance id, the first part of a unified message origin."""
    default_config_tmpl: dict | None = None
    adapter_display_name: str | None = None
    logo_path: str | None = None
    support_streaming_message: bool = False
    support_proactive_message: bool = True
    support_ui_interaction: bool = False
