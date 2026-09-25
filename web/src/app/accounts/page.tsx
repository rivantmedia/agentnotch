import { redirect } from "next/navigation";

/** Accounts are listed on the dashboard; each has its own page at /accounts/<key>. */
export default function AccountsIndex() {
  redirect("/dashboard");
}
