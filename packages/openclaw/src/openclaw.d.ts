// The parts of OpenClaw's plugin SDK the plugin uses, as release 2026.11.5 declares them, loosely typed.
// OpenClaw provides these modules to a plugin it loads; the package does not install OpenClaw, so these
// declarations stand in for it when the package is checked and built.

declare module "openclaw/plugin-sdk/channel-core" {
  export function createChatChannelPlugin(params: Record<string, unknown>): unknown;
  export function defineChannelPluginEntry(params: {
    id: string;
    name: string;
    description: string;
    plugin: unknown;
    registerFull?: (api: unknown) => void;
  }): unknown;
}

declare module "openclaw/plugin-sdk/channel-outbound" {
  export function defineChannelMessageAdapter(params: Record<string, unknown>): unknown;
}

declare module "openclaw/plugin-sdk/command-detection" {
  export function hasControlCommand(text: string, cfg: unknown): boolean;
}

declare module "openclaw/plugin-sdk/approval-gateway-runtime" {
  export function resolveApprovalOverGateway(params: Record<string, unknown>): Promise<unknown>;
}

declare module "openclaw/plugin-sdk/question-gateway-runtime" {
  export const questionGatewayRuntime: {
    resolveOption(params: Record<string, unknown>): Promise<unknown>;
  };
}

declare module "openclaw/plugin-sdk/native-command-registry" {
  export function listChatCommands(): Record<string, unknown>[];
  export function resolveCommandArgChoices(params: Record<string, unknown>): unknown;
}

declare module "openclaw/plugin-sdk/models-provider-runtime" {
  export function buildModelsProviderData(cfg: unknown, agentId: string): Promise<unknown>;
}

declare module "openclaw/plugin-sdk/session-store-runtime" {
  export function getSessionEntryAsync(params: { agentId: unknown; sessionKey: unknown }): Promise<unknown>;
}

declare module "openclaw/plugin-sdk/agent-runtime" {
  export function resolveAgentWorkspaceDir(cfg: unknown, agentId: string): string;
  export function resolveDefaultAgentId(cfg: unknown): string;
}
