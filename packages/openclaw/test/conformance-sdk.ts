// What the plugin imports from OpenClaw's plugin SDK, for test/conformance-harness.ts: the builders hand
// back what they are given, as test/sdk-stub.ts does, and the gateway's calls go to the scripted OpenClaw
// the harness sets on `globalThis`. Nothing here is OpenClaw's.

type Json = Record<string, unknown>;

/** The scripted OpenClaw, as test/conformance-harness.ts sets it. */
interface Scripted {
  resolveApproval(params: Json): Promise<unknown>;
  resolveQuestion(params: Json): Promise<unknown>;
}

function scripted(): Scripted {
  return (globalThis as unknown as { marmotConformance: Scripted }).marmotConformance;
}

export const createChatChannelPlugin = (plugin: Json): Json => plugin;
export const defineChannelPluginEntry = (entry: Json): Json => entry;
export const defineChannelMessageAdapter = (adapter: Json): Json => adapter;
export const hasControlCommand = (text: string): boolean => text.startsWith("/");
export const resolveApprovalOverGateway = (params: Json): Promise<unknown> => scripted().resolveApproval(params);
export const questionGatewayRuntime = { resolveOption: (params: Json): Promise<unknown> => scripted().resolveQuestion(params) };
export const getSessionEntryAsync = async (): Promise<unknown> => ({ sessionId: "s-1" });
export const listChatCommands = (): Json[] => [{ key: "new", description: "Start a new session.", textAliases: ["/new"] }];
export const resolveCommandArgChoices = (): unknown[] => [];
export const buildModelsProviderData = async (): Promise<unknown> => ({
  byProvider: { p: ["m"] },
  providers: ["p"],
  resolvedDefault: { provider: "p", model: "m" },
});
export const resolveAgentWorkspaceDir = (_cfg: unknown, agentId: string): string => `/workspaces/${agentId}`;
export const resolveDefaultAgentId = (): string => "main";
