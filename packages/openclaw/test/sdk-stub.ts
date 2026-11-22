// What the plugin imports from OpenClaw's plugin SDK, for test/plugin.test.ts: the builders hand back
// what they are given, so the test reaches the plugin's own adapters, and the runtime calls are not
// there. Nothing here is OpenClaw's.

type Json = Record<string, unknown>;
const absent = (): never => {
  throw new Error("not in the test's stand-in for OpenClaw");
};

export const createChatChannelPlugin = (plugin: Json): Json => plugin;
export const defineChannelPluginEntry = (entry: Json): Json => entry;
export const defineChannelMessageAdapter = (adapter: Json): Json => adapter;
export const hasControlCommand = (text: string): boolean => text.startsWith("/");
export const resolveApprovalOverGateway = absent;
export const questionGatewayRuntime = { resolveOption: absent };
export const getSessionEntryAsync = absent;
export const listChatCommands = (): Json[] => [];
export const resolveCommandArgChoices = absent;
export const buildModelsProviderData = absent;
export const resolveAgentWorkspaceDir = (_cfg: unknown, agentId: string): string => `/workspaces/${agentId}`;
export const resolveDefaultAgentId = (): string => "main";
