import { test, expect } from "./fixtures";
import { createClient } from "@supabase/supabase-js";
import { loadEnvConfig } from "@next/env";

import AxeBuilder from "@axe-core/playwright";

// Authenticated HTTP/cookie flows against the loopback-only auth/data double.
// Real PostgreSQL RLS and invitation transactions are verified in tests/database.
test("sign in, create an empty family, invite a member, enforce ownership, and sign out", async ({
  page,
  browser,
}) => {
  test.setTimeout(90000);
  loadEnvConfig(process.cwd());
  const admin = createClient(
    process.env.NEXT_PUBLIC_SUPABASE_URL!,
    process.env.SUPABASE_SERVICE_ROLE_KEY!,
    { auth: { persistSession: false } },
  );
  const credentials = {
    email: `kin-owner-${crypto.randomUUID()}@example.invalid`,
    password: crypto.randomUUID() + "aA1!",
  };
  const owner = await admin.auth.admin.createUser({
    ...credentials,
    email_confirm: true,
  });
  if (owner.error) throw owner.error;
  let familyId: string | undefined;
  let memberId: string | undefined;
  const guest = await browser.newContext({
    storageState: { cookies: [], origins: [] },
  });
  try {
    await page.context().clearCookies();
    await page.goto("/signin");
    await page.getByLabel("Email address").fill(credentials.email);
    await page
      .getByLabel("Password", { exact: true })
      .fill(credentials.password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/onboarding$/);
    await page.getByRole("button", { name: /Create a family space/ }).click();
    await page.getByLabel("Their name").fill("Browser Test Loved One");
    await page.getByRole("button", { name: "Continue", exact: true }).click();
    await page.getByLabel("Your name", { exact: true }).fill("Maya");
    await page.getByLabel(/Your relationship/).selectOption("granddaughter");
    await page
      .getByRole("button", { name: "Create family space", exact: true })
      .click();
    await expect(
      page.getByRole("link", { name: "Go to your memories" }),
    ).toBeVisible();
    const account = await page.request
      .get("/api/account")
      .then((r) => r.json());
    familyId = account.membership.family_id;
    await page.getByRole("link", { name: "Go to your memories" }).click();
    await expect(
      page.getByRole("heading", { name: "Your memories belong here." }),
    ).toBeVisible();
    const data = await page.request.get("/api/family").then((r) => r.json());
    expect(data.memories).toEqual([]);
    expect(data.faces).toEqual([]);
    expect(data.relatives).toHaveLength(1);
    expect(data.nodes).toHaveLength(1);
    const a = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
      .analyze();
    expect(a.violations).toEqual([]);
    await page.goto("/settings");
    await page.getByRole("button", { name: "Create invitation link" }).click();
    const link = await page.locator(".invite-output").innerText();
    expect(link).toContain("/onboarding?invite=");
    const email = `kin-invite-${crypto.randomUUID()}@example.invalid`,
      password = crypto.randomUUID() + "aA1!";
    const newUser = await admin.auth.admin.createUser({
      email,
      password,
      email_confirm: true,
    });
    if (newUser.error) throw newUser.error;
    memberId = newUser.data.user.id;
    const member = await guest.newPage();
    await member.goto(link);
    await expect(member).toHaveURL(/\/signin\?next=/);
    await member.getByLabel("Email address").fill(email);
    await member.getByLabel("Password", { exact: true }).fill(password);
    await member.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(member.getByLabel("Invitation link or code")).not.toHaveValue(
      "",
    );
    await member.getByRole("button", { name: "Continue", exact: true }).click();
    await member.getByLabel("Your name", { exact: true }).fill("Elena");
    await member.getByLabel(/Your relationship/).selectOption("daughter");
    await member
      .getByRole("button", { name: "Join family", exact: true })
      .click();
    await member.getByRole("link", { name: "Go to your memories" }).click();
    const shared = await member.request
      .get("/api/family")
      .then((r) => r.json());
    expect(shared.familyId).toBe(familyId);
    expect(shared.isOwner).toBe(false);
    expect(shared.relatives).toHaveLength(2);
    expect((await member.request.post("/api/invite")).status()).toBe(403);
    const forged = await member.request.delete(
      `/api/memories/22222222-2222-4222-8222-222222222222?contributor_id=${data.relativeId}`,
    );
    expect(forged.status()).toBe(403);
    await page.getByRole("button", { name: "Sign out", exact: true }).click();
    await expect(page).toHaveURL(/\/signin$/);
    expect((await page.request.get("/api/family")).status()).toBe(401);
  } finally {
    await guest.close();
    if (familyId) {
      for (const table of ["weaver_questions", "recall_events", "ingestion_receipts", "face_embeddings", "memories", "graph_edges", "graph_nodes"]) {
        const result = await admin.from(table).delete().eq("family_id", familyId);
        if (result.error) throw result.error;
      }
      const r = await admin.from("families").delete().eq("id", familyId);
      if (r.error) throw r.error;
    }
    if (memberId) {
      const r = await admin.auth.admin.deleteUser(memberId);
      if (r.error) throw r.error;
    }
    const removed = await admin.auth.admin.deleteUser(owner.data.user.id);
    if (removed.error) throw removed.error;
  }
});

test("a loved-one invitation joins without a relationship and opens loved-one navigation", async ({
  page,
  browser,
}) => {
  test.setTimeout(90000);
  loadEnvConfig(process.cwd());
  const admin = createClient(
    process.env.NEXT_PUBLIC_SUPABASE_URL!,
    process.env.SUPABASE_SERVICE_ROLE_KEY!,
    { auth: { persistSession: false } },
  );
  const migration = await admin.from("family_members").select("role").limit(1);
  test.skip(
    !!migration.error,
    "Apply migration 010 to run the loved-one account test.",
  );
  const ownerCredentials = {
    email: `kin-loved-owner-${crypto.randomUUID()}@example.invalid`,
    password: crypto.randomUUID() + "aA1!",
  };
  const lovedCredentials = {
    email: `kin-loved-${crypto.randomUUID()}@example.invalid`,
    password: crypto.randomUUID() + "aA1!",
  };
  const ids: string[] = [];
  let familyId: string | undefined;
  const guest = await browser.newContext({
    storageState: { cookies: [], origins: [] },
  });
  try {
    for (const credentials of [ownerCredentials, lovedCredentials]) {
      const r = await admin.auth.admin.createUser({
        ...credentials,
        email_confirm: true,
      });
      if (r.error) throw r.error;
      ids.push(r.data.user.id);
    }
    await page.context().clearCookies();
    await page.goto("/signin");
    await page.getByLabel("Email address").fill(ownerCredentials.email);
    await page
      .getByLabel("Password", { exact: true })
      .fill(ownerCredentials.password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/onboarding$/);
    // Finish the initial membership read before creating this fixture via API.
    await expect(page.getByRole("heading", { name: "Welcome to Kin.", exact: true })).toBeVisible();
    const created = await page.request.post("/api/onboarding", {
      data: {
        mode: "create",
        name: "Maya",
        relationship: "granddaughter",
        lovedOne: "Rosa",
      },
    });
    expect(created.status()).toBe(200);
    familyId = (await created.json()).familyId;
    await page.goto("/settings");
    await page
      .getByRole("button", { name: "Invite Rosa", exact: true })
      .click();
    const link = await page
      .getByLabel("Invitation for Rosa", { exact: true })
      .innerText();
    const rosa = await guest.newPage();
    await rosa.goto(link);
    await expect(rosa).toHaveURL(/\/signin\?next=/);
    await rosa.getByLabel("Email address").fill(lovedCredentials.email);
    await rosa
      .getByLabel("Password", { exact: true })
      .fill(lovedCredentials.password);
    await rosa.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(
      rosa.getByRole("heading", { name: "Welcome, Rosa." }),
    ).toBeVisible();
    await expect(rosa.getByLabel(/Your relationship/)).toHaveCount(0);
    await rosa
      .getByRole("button", { name: "Join and open Kin", exact: true })
      .click();
    await expect(rosa).toHaveURL(/\/wearer$/);
    await expect(
      rosa.getByRole("button", { name: "Open camera", exact: true }),
    ).toBeVisible();
    const membership = await rosa.request
      .get("/api/account")
      .then((r) => r.json());
    expect(membership.membership.role).toBe("loved_one");
    expect(membership.membership.relative_id).toBeNull();
    const data = await rosa.request.get("/api/family").then((r) => r.json());
    expect(data.relatives).toHaveLength(1);
    expect(data.familyId).toBe(familyId);
    for (const path of [
      "/api/memories/photo",
      "/api/memories/story",
      "/api/faces/enroll",
      "/api/weaver/answer",
      "/api/weaver/run",
      "/api/invite",
    ])
      expect((await rosa.request.post(path, { data: {} })).status()).toBe(403);
    for (const path of ["/family", "/stage"]) {
      await rosa.goto(path);
      await expect(rosa).toHaveURL(/\/wearer$/);
    }
    await rosa
      .getByRole("link", { name: "Your settings", exact: true })
      .click();
    await expect(
      rosa.getByRole("heading", { name: "Settings", exact: true }),
    ).toBeVisible();
    await expect(
      rosa.getByRole("button", { name: /Invite|Create invitation/ }),
    ).toHaveCount(0);
    await expect(
      rosa.getByRole("navigation", { name: "Main navigation" }).getByRole("link"),
    ).toHaveText(["Recognize", "Stories", "Record"]);
    const a = await new AxeBuilder({ page: rosa })
      .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
      .analyze();
    expect(a.violations).toEqual([]);
    await page.reload();
    await expect(
      page.getByText("Rosa is connected.", { exact: false }),
    ).toBeVisible();
    expect(
      (
        await page.request.post("/api/invite", { data: { role: "loved_one" } })
      ).status(),
    ).toBe(409);
    const token = new URL(link).searchParams.get("invite");
    expect(
      (await rosa.request.get(`/api/invite?token=${token}`)).status(),
    ).toBe(404);
  } finally {
    await guest.close();
    if (familyId) {
      for (const table of ["weaver_questions", "recall_events", "ingestion_receipts", "face_embeddings", "memories", "graph_edges", "graph_nodes"]) {
        const result = await admin.from(table).delete().eq("family_id", familyId);
        if (result.error) throw result.error;
      }
      const r = await admin.from("families").delete().eq("id", familyId);
      if (r.error) throw r.error;
    }
    for (const id of ids) {
      const r = await admin.auth.admin.deleteUser(id);
      if (r.error) throw r.error;
    }
  }
});
