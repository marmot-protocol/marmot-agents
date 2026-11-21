/**
 * The connector's own tool, `send_file`, which lets Claude Code send a file to the chat (D-090). It is an
 * in-process MCP server the SDK runs, so Claude Code sees it as `mcp__marmot__send_file`. The connector asks
 * the core to send the file; the core checks it is in a folder its settings allow, and sends it as an
 * encrypted attachment.
 */

import { createSdkMcpServer, tool } from "@anthropic-ai/claude-agent-sdk";
import { z } from "zod";

/** The tool as Claude Code names it, for `allowedTools`: it needs nobody's approval, since the core decides. */
export const SEND_FILE_TOOL = "mcp__marmot__send_file";

const DESCRIPTION =
  "Send a file from the working folder to the chat, as an attachment the people in the chat can open. " +
  "Give its path, relative to the working folder or absolute.";

/** What the tool does with a path: sends it, and says how it went in words for the model. */
export type SendFile = (path: string) => Promise<{ text: string; failed: boolean }>;

/** Makes the MCP server that offers the tool to one query. */
export type FileServer = (send: SendFile) => unknown;

/** The SDK's in-process MCP server with the one tool. */
export const fileServer: FileServer = (send) =>
  createSdkMcpServer({
    name: "marmot",
    version: "1.0.0",
    tools: [
      tool("send_file", DESCRIPTION, { path: z.string().describe("The file to send.") }, async ({ path }) => {
        const { text, failed } = await send(path);
        return { content: [{ type: "text", text }], isError: failed };
      }),
    ],
  });
