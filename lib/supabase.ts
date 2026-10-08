import { createBrowserClient } from "@supabase/ssr";
import type { SupabaseClient } from "@supabase/supabase-js";
import { CONFIG } from "./config";

let browserClient: SupabaseClient | null = null;

export function getAnonClient(): SupabaseClient | null {
  const url = process.env.NEXT_PUBLIC_SUPABASE_URL;
  const key = process.env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY || process.env.NEXT_PUBLIC_SUPABASE_ANON_KEY;
  if (!url || !key) return null;
  if (!browserClient) browserClient = createBrowserClient(url, key);
  return browserClient;
}

export const FAMILY_ID = CONFIG.familyId;
