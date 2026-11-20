// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ApiUserToolServer } from "@/lib/api/types";
import type { ServerFields } from "../../lib/user-servers";
import { type FormOutcome, ServerForm } from "./server-form";

afterEach(cleanup);

const ALL = ["none", "bearer", "api_key", "oauth2"] as const;
const SECRET = "tok-9f3a-this-is-a-secret-value";

const type = (label: string | RegExp, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
const pick = (kind: string) =>
  fireEvent.change(screen.getByLabelText("Authentication"), { target: { value: kind } });

function renderForm(
  over: Partial<Parameters<typeof ServerForm>[0]> = {},
  outcome: FormOutcome = { ok: true },
) {
  const onSubmit = vi.fn<(f: ServerFields) => Promise<FormOutcome>>(async () => outcome);
  const utils = render(<ServerForm mode="add" kinds={ALL} onSubmit={onSubmit} {...over} />);
  return { ...utils, onSubmit };
}

const server = (over: Partial<ApiUserToolServer> = {}): ApiUserToolServer => ({
  id: "u-abcdefghij",
  name: "Search",
  own: true,
  status: "ready",
  url: "https://mcp.example.com/mcp",
  auth: { kind: "bearer" },
  state: "connected",
  hasCredential: true,
  revision: "3",
  ...over,
});

describe("ServerForm: adding", () => {
  it("offers the kinds the API listed and no others, each in words", () => {
    renderForm({ kinds: ["none", "oauth2"] });
    const options = [...(screen.getByLabelText("Authentication") as HTMLSelectElement).options];
    expect(options.map((o) => o.textContent)).toEqual(["No authentication", "OAuth sign-in"]);
  });

  it("asks for a secret only for the kinds that take one, in a password field that autofill leaves alone", () => {
    renderForm();
    expect(screen.queryByLabelText("Token")).toBeNull();
    pick("bearer");
    const token = screen.getByLabelText("Token") as HTMLInputElement;
    expect(token.type).toBe("password");
    expect(token.autocomplete).toBe("new-password");
    expect(token.value).toBe("");
    pick("api_key");
    expect(screen.getByLabelText("API key")).toBeTruthy();
    expect(screen.getByLabelText("Header name")).toBeTruthy();
    pick("oauth2");
    expect(screen.queryByLabelText("API key")).toBeNull();
    expect(screen.getByLabelText("Scopes (optional)")).toBeTruthy();
    expect(screen.getByLabelText("Client ID (optional)")).toBeTruthy();
  });

  it("says what each kind asks of the person", () => {
    renderForm();
    pick("oauth2");
    expect(screen.getByText(/sign in at the server.s own authorization service/)).toBeTruthy();
  });

  it("is not sent until the name, the address and what the kind needs are there", () => {
    renderForm();
    const submit = screen.getByRole("button", { name: "Add server" }) as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    type("Name", "Search");
    type("Address", "https://mcp.example.com/mcp");
    pick("bearer");
    expect(submit.disabled).toBe(true);
    type("Token", SECRET);
    expect(submit.disabled).toBe(false);
  });

  it("sends what was typed and clears the secret the moment it is sent, whatever comes of it", async () => {
    const { onSubmit } = renderForm({}, { ok: false, message: "Nope." });
    type("Name", "Search");
    type("Address", "https://mcp.example.com/mcp");
    pick("bearer");
    type("Token", SECRET);
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(onSubmit.mock.calls[0]?.[0]).toMatchObject({
      name: "Search",
      kind: "bearer",
      secret: SECRET,
    });
    // cleared at once, and a refusal does not bring it back: the person types it again
    await waitFor(() => expect(screen.getByRole("alert").textContent).toBe("Nope."));
    expect((screen.getByLabelText("Token") as HTMLInputElement).value).toBe("");
    expect(document.body.innerHTML).not.toContain(SECRET);
    // the rest of what was typed is kept
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Search");
  });

  it("starts again, empty, after a server is added", async () => {
    const { onSubmit } = renderForm();
    type("Name", "Search");
    type("Address", "https://mcp.example.com/mcp");
    fireEvent.click(screen.getByRole("button", { name: "Add server" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalled());
    await waitFor(() => expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe(""));
  });
});

describe("ServerForm: changing", () => {
  it("is filled in with the server, and never with a secret", () => {
    renderForm({ mode: "edit", server: server() });
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Search");
    expect((screen.getByLabelText("Address") as HTMLInputElement).value).toBe(
      "https://mcp.example.com/mcp",
    );
    expect((screen.getByLabelText("New token") as HTMLInputElement).value).toBe("");
    expect(screen.getByText(/Leave it empty to keep the current one/)).toBeTruthy();
  });

  it("keeps a kind the deployment no longer lists, so the server is shown as it is", () => {
    renderForm({ mode: "edit", kinds: ["none"], server: server() });
    const options = [...(screen.getByLabelText("Authentication") as HTMLSelectElement).options];
    expect(options.map((o) => o.value)).toEqual(["bearer", "none"]);
  });

  it("warns before a new address or kind makes the server forget its credential, and not for a rename", () => {
    renderForm({ mode: "edit", server: server() });
    expect(screen.queryByRole("status")).toBeNull();
    type("Name", "Renamed");
    expect(screen.queryByRole("status")).toBeNull();
    type("Address", "https://other.example.com/mcp");
    expect(screen.getByRole("status").textContent).toMatch(/forget its current key or sign-in/);
  });

  it("is only the secret for a new key", () => {
    renderForm({ mode: "key", server: server() });
    expect(screen.queryByLabelText("Name")).toBeNull();
    expect(screen.queryByLabelText("Address")).toBeNull();
    expect(screen.getByLabelText("New token")).toBeTruthy();
    expect((screen.getByRole("button", { name: "Save key" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it("can be given up", () => {
    const onCancel = vi.fn();
    renderForm({ mode: "edit", server: server(), onCancel });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalled();
  });
});
