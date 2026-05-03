/**
 * Tier-4 Phase 3 — Playwright E2E conformance suite.
 *
 * One spec per F-property (F1, F2, F3, F4, F6, F7, F8, F9), mapped to
 * UI flows. The verified backend (Verus + TLC) checks these properties
 * against the abstract spec; this suite checks that the deployed UI
 * exposes the same behavior to a real browser.
 *
 * Per K4: every handle is namespaced with the run id so consecutive
 * runs against shared production state don't collide.
 */

import { test, expect } from "@playwright/test";
import {
  ns,
  apiCreateUser,
  apiPostTweet,
  apiFollow,
  uiLogin,
} from "./helpers";

test.describe("F-property conformance (UI layer)", () => {
  test("F1: alice follows bob, posts; carol posts; alice's timeline contains alice+bob, NOT carol", async ({
    page,
    request,
  }) => {
    const alice = ns("f1alice");
    const bob = ns("f1bob");
    const carol = ns("f1carol");
    await apiCreateUser(request, alice);
    await apiCreateUser(request, bob);
    await apiCreateUser(request, carol);
    await apiPostTweet(request, alice, `alice tweet ${ns("")}`);
    await apiPostTweet(request, bob, `bob tweet ${ns("")}`);
    await apiPostTweet(request, carol, `carol tweet ${ns("")}`);
    await apiFollow(request, alice, bob);

    await uiLogin(page, alice);
    const body = await page.locator("body").innerText();
    expect(body).toContain(`alice tweet ${ns("")}`);
    expect(body).toContain(`bob tweet ${ns("")}`);
    expect(body).not.toContain(`carol tweet ${ns("")}`); // F1
  });

  test("F2: timeline ordered by (created_at desc, id desc)", async ({
    page,
    request,
  }) => {
    const u = ns("f2u");
    await apiCreateUser(request, u);
    const t1 = await apiPostTweet(request, u, `${ns("t2-1")}`);
    const t2 = await apiPostTweet(request, u, `${ns("t2-2")}`);
    const t3 = await apiPostTweet(request, u, `${ns("t2-3")}`);
    expect(t2.id).toBeGreaterThan(t1.id);
    expect(t3.id).toBeGreaterThan(t2.id); // F8 also covered

    await uiLogin(page, u);
    const body = await page.locator("body").innerText();
    const i1 = body.indexOf(ns("t2-1"));
    const i2 = body.indexOf(ns("t2-2"));
    const i3 = body.indexOf(ns("t2-3"));
    expect(i1).toBeGreaterThanOrEqual(0);
    expect(i2).toBeGreaterThanOrEqual(0);
    expect(i3).toBeGreaterThanOrEqual(0);
    // Highest-id tweet appears first (DOM order = display order = newest first)
    expect(i3).toBeLessThan(i2); // t3 before t2 in DOM
    expect(i2).toBeLessThan(i1); // t2 before t1 in DOM
  });

  test("F3: follow is idempotent — bob's tweets appear once, not twice", async ({
    page,
    request,
  }) => {
    const alice = ns("f3alice");
    const bob = ns("f3bob");
    const marker = `bob-once ${ns("")}`;
    await apiCreateUser(request, alice);
    await apiCreateUser(request, bob);
    await apiPostTweet(request, bob, marker);
    await apiFollow(request, alice, bob);
    await apiFollow(request, alice, bob); // duplicate
    await apiFollow(request, alice, bob); // triplicate

    await uiLogin(page, alice);
    const matches = (await page.locator("body").innerText()).split(marker)
      .length - 1;
    expect(matches).toBe(1); // F3
  });

  test("F4: self-follow rejected (4xx)", async ({ request }) => {
    const alice = ns("f4alice");
    await apiCreateUser(request, alice);
    const r = await request.post("/follow", {
      data: { from: alice, to: alice },
    });
    expect(r.status()).toBeGreaterThanOrEqual(400);
    expect(r.status()).toBeLessThan(500);
    const body = await r.json();
    expect(body.error).toBe("self_follow_forbidden");
  });

  test("F6: tweet from unknown author rejected (4xx)", async ({ request }) => {
    const r = await request.post("/tweets", {
      data: { author: ns("f6ghost"), text: "i should not exist" },
    });
    expect(r.status()).toBeGreaterThanOrEqual(400);
    expect(r.status()).toBeLessThan(500);
    expect((await r.json()).error).toBe("unknown_user");
  });

  test("F7: clock is non-decreasing across two posts", async ({ request }) => {
    const u = ns("f7u");
    await apiCreateUser(request, u);
    const a = await apiPostTweet(request, u, `f7a ${ns("")}`);
    const b = await apiPostTweet(request, u, `f7b ${ns("")}`);
    expect((b as any).created_at).toBeGreaterThanOrEqual((a as any).created_at);
  });

  test("F8: tweet IDs strictly increase", async ({ request }) => {
    const u = ns("f8u");
    await apiCreateUser(request, u);
    const ids: number[] = [];
    for (let i = 0; i < 5; i++) {
      const t = await apiPostTweet(request, u, `f8 ${i} ${ns("")}`);
      ids.push((t as any).id);
    }
    for (let i = 1; i < ids.length; i++) {
      expect(ids[i]).toBeGreaterThan(ids[i - 1]);
    }
  });

  test("F9: follow unknown user rejected (4xx)", async ({ request }) => {
    const u = ns("f9u");
    await apiCreateUser(request, u);
    const r = await request.post("/follow", {
      data: { from: u, to: ns("f9ghost") },
    });
    expect(r.status()).toBeGreaterThanOrEqual(400);
    expect(r.status()).toBeLessThan(500);
    expect((await r.json()).error).toBe("unknown_user");
  });
});
