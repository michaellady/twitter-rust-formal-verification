import type { Page, APIRequestContext } from "@playwright/test";

// Per-run namespace (K4): every test handle gets this suffix so runs
// against shared production state don't collide.
export const RUN_ID =
  process.env.PLAYWRIGHT_RUN_ID ??
  `local${Date.now().toString(36).slice(-6)}`;

export const ns = (h: string) => `${h}_${RUN_ID}`;

export interface CreatedUser {
  handle: string;
  id: number;
}

/** Use the JSON API directly to register users for test setup. */
export async function apiCreateUser(
  request: APIRequestContext,
  handle: string,
): Promise<CreatedUser> {
  const r = await request.post("/users", { data: { handle } });
  if (![200, 201, 409].includes(r.status())) {
    throw new Error(`apiCreateUser(${handle}) → ${r.status()} ${await r.text()}`);
  }
  if (r.status() === 409) {
    return { handle, id: -1 };
  }
  return r.json();
}

export async function apiPostTweet(
  request: APIRequestContext,
  author: string,
  text: string,
): Promise<{ id: number; created_at: number }> {
  const r = await request.post("/tweets", { data: { author, text } });
  if (![200, 201].includes(r.status())) {
    throw new Error(`apiPostTweet(${author}) → ${r.status()} ${await r.text()}`);
  }
  return r.json();
}

export async function apiFollow(
  request: APIRequestContext,
  from: string,
  to: string,
): Promise<void> {
  const r = await request.post("/follow", { data: { from, to } });
  if (![204, 200].includes(r.status())) {
    throw new Error(`apiFollow(${from}→${to}) → ${r.status()} ${await r.text()}`);
  }
}

/** Log in as the given handle via the UI: hits /register over fetch
 *  (which sets the cookie on the browsing context), then navigates to /.
 *  We use fetch instead of clicking the form button because Playwright's
 *  click handling of `<button formaction>` can race with the redirect.
 */
export async function uiLogin(page: Page, handle: string): Promise<void> {
  await page.goto("/");
  await page.evaluate(async (h) => {
    const body = new URLSearchParams({ handle: h }).toString();
    const r = await fetch("/register", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body,
      redirect: "manual", // we just want the cookie; don't follow
    });
    if (r.status >= 400) {
      throw new Error(`register failed: ${r.status}`);
    }
  }, handle);
  await page.goto("/");
}
