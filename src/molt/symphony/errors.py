from __future__ import annotations


class SymphonyError(RuntimeError):
    """Base Symphony exception."""


class MissingWorkflowFileError(SymphonyError):
    pass


class WorkflowParseError(SymphonyError):
    pass


class WorkflowFrontMatterNotMapError(SymphonyError):
    pass


class TemplateParseError(SymphonyError):
    pass


class TemplateRenderError(SymphonyError):
    pass


class ConfigValidationError(SymphonyError):
    pass


class TrackerError(SymphonyError):
    pass


class WorkspaceError(SymphonyError):
    pass


class HookError(WorkspaceError):
    pass


class AgentRunnerError(SymphonyError):
    pass


class ResponseTimeoutError(AgentRunnerError):
    pass


class TurnTimeoutError(AgentRunnerError):
    pass


class TurnFailedError(AgentRunnerError):
    pass


class TurnCancelledError(AgentRunnerError):
    pass


class TurnInputRequiredError(AgentRunnerError):
    pass


class UnsupportedToolCallError(AgentRunnerError):
    pass
