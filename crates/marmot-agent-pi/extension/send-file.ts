// The Pi driver's own extension: gives Pi a tool, `send_file`, that sends a file to the chat (D-090).
//
// The driver writes this file out and starts each chat's Pi with `--extension <it>`. Pi's RPC mode lets a
// client add no tool of its own, so the tool asks the driver through Pi's extension UI, an `input` dialog
// with the title below. The driver takes that dialog as the request, hands it to the core, and answers it
// with how the send went; it never reaches the chat as a question. The path goes as the model gave it,
// but for a leading `@`, which Pi's own tools also drop; the driver makes a relative one whole.

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** The title the driver knows the request by. */
const REQUEST = "marmot-agent send_file";

export default function (pi: ExtensionAPI) {
  pi.registerTool({
    name: "send_file",
    label: "Send file",
    description:
      "Send a file from the working folder to the chat, as an attachment the people in the chat can open. Give its path, relative to the working folder or absolute.",
    promptSnippet: "Send a file to the chat as an attachment",
    parameters: {
      type: "object",
      properties: { path: { type: "string", description: "The file to send." } },
      required: ["path"],
      additionalProperties: false,
    },
    async execute(_toolCallId, params, signal, _onUpdate, ctx) {
      const path = String(params.path);
      const reply = await ctx.ui.input(REQUEST, JSON.stringify({ path }), { signal });
      let answer: { sent?: unknown; text?: unknown } = {};
      try {
        answer = JSON.parse(reply ?? "{}");
      } catch {}
      const text = typeof answer.text === "string" ? answer.text : "Not sent.";
      return { content: [{ type: "text", text }], details: undefined, isError: answer.sent !== true };
    },
  });
}
