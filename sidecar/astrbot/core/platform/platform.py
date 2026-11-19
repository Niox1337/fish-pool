from astrbot._unsupported import unsupported


class Platform:
    """Platform adapters run in the fish node, so plugins cannot implement or drive one."""

    def __init__(self, *args, **kwargs) -> None:
        unsupported("Platform")
