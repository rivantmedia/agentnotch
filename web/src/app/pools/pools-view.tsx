"use client";

import Link from "next/link";
import { useId, useState, type FormEvent } from "react";

import { accountSubtitle, accountTitle, plural } from "~/lib/format";
import {
  formatPoolCode,
  normalizePoolCode,
  POOL_CODE_EXAMPLE,
} from "~/server/services/pool-code";
import { describeError, errorCode } from "~/trpc/errors";
import { api, type RouterOutputs } from "~/trpc/react";

import { ConfirmAction } from "../_components/confirm-action";
import { CopyButton } from "../_components/copy-button";
import { QueryBoundary } from "../_components/query-boundary";
import { DateTime, RelativeTime } from "../_components/time";
import {
  Badge,
  Callout,
  LoadingBlock,
  PageHeader,
  SectionHeading,
  Skeleton,
} from "../_components/ui";

type Pool = RouterOutputs["pools"]["list"][number];
type AccountInfo = {
  label: string | null;
  email: string | null;
  organizationName: string | null;
  plan: string | null;
};

/** Said before anything that shares data: what a pool member sees, and what they don't. */
const SHARING_WARNING =
  "Everyone in a pool sees every member's sessions, session summaries and usage on this Claude account, including yours, and each member's email address. Nothing from your other accounts is shared.";

function useInvalidatePools() {
  const utils = api.useUtils();
  return () =>
    Promise.all([
      utils.pools.list.invalidate(),
      utils.accounts.list.invalidate(),
    ]);
}

export function PoolsView() {
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-10 px-4 py-8 sm:py-10">
      <PageHeader
        title="Pools"
        description="Share a Claude account's history with the other people who use it. Each pool covers one account, and joins by a code you send them."
      />

      <div className="grid items-start gap-8 lg:grid-cols-[minmax(0,1fr)_22rem]">
        <section aria-labelledby="share-title" className="flex flex-col gap-4">
          <SectionHeading
            id="share-title"
            title="Share your accounts"
            description="Accounts your Mac has synced. Create a code for one, then send it to the people who use that account too."
          />
          <QueryBoundary
            what="your pools"
            fallback={
              <LoadingBlock label="Loading your accounts and pools…">
                <div className="flex flex-col gap-4" aria-hidden="true">
                  {[0, 1].map((i) => (
                    <div key={i} className="card flex flex-col gap-3 p-5">
                      <Skeleton className="h-4 w-44" />
                      <Skeleton className="h-3 w-64 max-w-full" />
                      <Skeleton className="h-9 w-40" />
                    </div>
                  ))}
                </div>
              </LoadingBlock>
            }
          >
            <ShareList />
          </QueryBoundary>
        </section>

        <aside aria-labelledby="join-title" className="lg:sticky lg:top-20">
          <JoinForm />
        </aside>
      </div>

      <section aria-labelledby="joined-title" className="flex flex-col gap-4">
        <SectionHeading
          id="joined-title"
          title="Pools you joined"
          description="Accounts other people share with you. Leave a pool to stop sharing in both directions."
        />
        <QueryBoundary
          what="the pools you joined"
          fallback={
            <LoadingBlock label="Loading the pools you joined…">
              <div className="card flex flex-col gap-3 p-5" aria-hidden="true">
                <Skeleton className="h-4 w-44" />
                <Skeleton className="h-3 w-64 max-w-full" />
              </div>
            </LoadingBlock>
          }
        >
          <JoinedPools />
        </QueryBoundary>
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Sharing an account you synced

function ShareList() {
  const [pools] = api.pools.list.useSuspenseQuery();
  const [accounts] = api.accounts.list.useSuspenseQuery();
  const created = pools.filter((p) => p.role === "creator");
  const synced = accounts.filter((a) => a.syncedByViewer);
  const keys = [
    ...new Set([
      ...synced.map((a) => a.key),
      ...created.map((p) => p.accountKey),
    ]),
  ];

  if (keys.length === 0) {
    return (
      <div className="card flex flex-col gap-2 p-5 text-sm text-ink-2">
        <p className="font-medium text-ink">No synced accounts yet.</p>
        <p>
          You can share an account once your Mac has synced it. Turn on
          &ldquo;Sync sessions and usage&rdquo; in Agent Notch (Settings &gt;
          Claude Code &gt; Cloud); see{" "}
          <Link href="/settings" className="link">
            Settings
          </Link>{" "}
          for this website&apos;s address.
        </p>
      </div>
    );
  }

  return (
    <ul className="flex flex-col gap-4">
      {keys.map((key) => {
        const mine = created.find((p) => p.accountKey === key) ?? null;
        const account: AccountInfo =
          synced.find((a) => a.key === key) ?? mine!.account;
        return (
          <li key={key}>
            <ShareCard
              accountKey={key}
              account={account}
              pool={mine}
              joined={pools.filter(
                (p) => p.role === "member" && p.accountKey === key,
              )}
            />
          </li>
        );
      })}
    </ul>
  );
}

function ShareCard({
  accountKey,
  account,
  pool,
  joined,
}: {
  accountKey: string;
  account: AccountInfo;
  /** The viewer's own pool on this account (one per account), if they made one. */
  pool: Pool | null;
  /** Other people's pools on this account that the viewer joined. */
  joined: Pool[];
}) {
  const subtitle = accountSubtitle(account);

  return (
    <article className="card flex flex-col gap-5 p-5">
      <header className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="font-semibold break-words">
            <Link href={`/accounts/${accountKey}`} className="hover:underline">
              {accountTitle(account)}
            </Link>
          </h3>
          {subtitle ? (
            <p className="text-sm break-words text-ink-2">{subtitle}</p>
          ) : null}
        </div>
        {account.plan ? <Badge>{account.plan}</Badge> : null}
      </header>

      {pool ? (
        <OwnPool pool={pool} account={account} />
      ) : (
        <div className="flex flex-col gap-3">
          <p className="text-sm text-ink-2">
            You haven&apos;t shared this account.
          </p>
          <CreateCode
            accountKey={accountKey}
            account={account}
            label="Create a share code"
          />
        </div>
      )}

      {joined.length > 0 ? (
        <p className="border-t border-line pt-4 text-sm text-ink-2">
          You&apos;re also in{" "}
          {joined.map((other, i) => (
            <span key={other.id}>
              {i > 0 ? " and " : null}
              <span className="font-medium break-all text-ink">
                {other.createdBy.displayName}
              </span>
              &apos;s pool
            </span>
          ))}{" "}
          for this account; see Pools you joined, below.
        </p>
      ) : null}
    </article>
  );
}

/** Creates a code: the first one makes the pool, later ones replace a lapsed code in it. */
function CreateCode({
  accountKey,
  account,
  label,
}: {
  accountKey: string;
  account: AccountInfo;
  label: string;
}) {
  const invalidate = useInvalidatePools();
  const create = api.pools.create.useMutation({ onSuccess: invalidate });
  return (
    <>
      <ConfirmAction
        trigger={label}
        triggerClassName="btn btn-primary self-start"
        tone="primary"
        question={
          <p className="font-medium">
            Create a code for {accountTitle(account)}?
          </p>
        }
        confirmLabel="Create code"
        pending={create.isPending}
        onConfirm={() => create.mutateAsync({ accountKey })}
      >
        <Callout tone="warning" title="Pool members see each other's sessions">
          {SHARING_WARNING} The code works for 7 days. Only share it with people
          who use this account.
        </Callout>
      </ConfirmAction>
      <MutationError error={create.error} what="create the code" />
    </>
  );
}

function OwnPool({ pool, account }: { pool: Pool; account: AccountInfo }) {
  const invalidate = useInvalidatePools();
  const revoke = api.pools.revoke.useMutation({ onSuccess: invalidate });
  const leave = api.pools.leave.useMutation({ onSuccess: invalidate });
  const codeId = useId();
  const share = pool.share;
  const code = share?.code ? formatPoolCode(share.code) : null;

  return (
    <div className="flex flex-col gap-5">
      {share && code ? (
        <div className="flex flex-col gap-2">
          <p id={codeId} className="text-xs font-medium text-ink-2">
            Share code
          </p>
          <div className="flex flex-wrap items-center gap-3">
            <output
              aria-labelledby={codeId}
              className="rounded-lg border border-line bg-surface-2 px-3.5 py-2 font-mono text-lg font-semibold tracking-[0.15em] break-all select-all"
            >
              {code}
            </output>
            <CopyButton value={code} label="Copy code" describedBy={codeId} />
          </div>
          <p className="text-xs text-ink-3">
            Works until <DateTime date={share.expiresAt} kind="day" /> for
            anyone signed in, unless you revoke it first.
          </p>
        </div>
      ) : (
        <div className="flex flex-col gap-3">
          <p className="text-sm text-ink-2">
            {share?.status === "revoked"
              ? "The code was revoked, so nobody new can join."
              : "The code expired, so nobody new can join."}{" "}
            {pool.members.length > 1
              ? "The people already in the pool stay. A new code lets more people join the same pool."
              : "Create a new code to invite people."}
          </p>
          <CreateCode
            accountKey={pool.accountKey}
            account={account}
            label="Create a new code"
          />
        </div>
      )}

      <MemberList pool={pool} />

      <div className="flex flex-col gap-3 border-t border-line pt-4">
        <div className="flex flex-wrap gap-2">
          {code ? (
            <ConfirmAction
              trigger="Revoke code"
              question={
                <p>
                  <span className="font-medium">Revoke {code}?</span> Nobody new
                  can join with it. People already in the pool stay until you
                  remove them; you can create a new code afterwards.
                </p>
              }
              confirmLabel="Revoke code"
              pending={revoke.isPending}
              onConfirm={() => revoke.mutateAsync({ poolId: pool.id })}
            />
          ) : null}
          <ConfirmAction
            trigger="Stop sharing"
            triggerClassName="btn btn-ghost btn-sm"
            question={
              <p>
                <span className="font-medium">
                  Stop sharing and delete this pool?
                </span>{" "}
                Everyone in it stops seeing each other&apos;s sessions on this
                account, and the code stops working.
              </p>
            }
            confirmLabel="Delete pool"
            pending={leave.isPending}
            onConfirm={() => leave.mutateAsync({ poolId: pool.id })}
          />
        </div>
        <MutationError error={revoke.error} what="revoke the code" />
        <MutationError error={leave.error} what="delete the pool" />
      </div>
    </div>
  );
}

/** The people in a pool; its creator can remove anyone but themself. */
function MemberList({ pool }: { pool: Pool }) {
  const invalidate = useInvalidatePools();
  const remove = api.pools.removeMember.useMutation({ onSuccess: invalidate });
  const canRemove = pool.role === "creator";
  const others = pool.members.filter((m) => !m.isViewer);

  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs font-medium text-ink-2">
        Members ({pool.members.length})
      </p>
      <ul className="flex flex-col divide-y divide-line rounded-lg border border-line">
        {pool.members.map((member) => (
          <li
            key={member.userId}
            className="flex flex-wrap items-center justify-between gap-2 px-3 py-2.5"
          >
            <div className="flex min-w-0 flex-col">
              <span className="text-sm font-medium break-all">
                {member.displayName}
                {member.isViewer ? (
                  <span className="font-normal text-ink-2"> (you)</span>
                ) : null}
              </span>
              {member.name ? (
                <span className="text-xs break-words text-ink-2">
                  {member.name}
                </span>
              ) : null}
              <span className="text-xs text-ink-3">
                {member.isCreator ? "Created the pool" : "Joined"}{" "}
                <RelativeTime date={member.joinedAt} />
              </span>
            </div>
            {canRemove && !member.isViewer ? (
              <ConfirmAction
                trigger="Remove"
                triggerClassName="btn btn-ghost btn-sm"
                question={
                  <p>
                    <span className="font-medium break-all">
                      Remove {member.displayName}?
                    </span>{" "}
                    You stop seeing each other&apos;s sessions on this account.
                    The current code stops working too, so they can&apos;t use
                    it to come back; create a new code to invite anyone else.
                  </p>
                }
                confirmLabel="Remove"
                pending={
                  remove.isPending && remove.variables?.userId === member.userId
                }
                onConfirm={() =>
                  remove.mutateAsync({ poolId: pool.id, userId: member.userId })
                }
              />
            ) : null}
          </li>
        ))}
      </ul>
      {canRemove && others.length === 0 ? (
        <p className="text-sm text-ink-2">
          Nobody has joined yet. Send the code to the people who use this
          account.
        </p>
      ) : null}
      <MutationError error={remove.error} what="remove that member" />
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Joining someone else's pool

type JoinResult =
  | { kind: "joined" | "already"; poolId: string; accountKey: string }
  | { kind: "error"; message: string };

function JoinForm() {
  const utils = api.useUtils();
  const invalidate = useInvalidatePools();
  const join = api.pools.join.useMutation();
  const [code, setCode] = useState("");
  const [result, setResult] = useState<JoinResult | null>(null);
  const inputId = useId();
  const hintId = useId();

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const normalized = normalizePoolCode(code);
    if (!normalized) {
      setResult({ kind: "error", message: NOT_A_CODE });
      return;
    }
    setResult(null);
    try {
      const outcome = await join.mutateAsync({ code: normalized });
      await invalidate();
      setResult({
        kind: outcome.joined ? "joined" : "already",
        poolId: outcome.poolId,
        accountKey: outcome.accountKey,
      });
      if (outcome.joined) setCode("");
    } catch (error) {
      setResult({ kind: "error", message: joinErrorMessage(error) });
    }
  }

  // Read once a join finished (after the refetch above), never while hydrating.
  const joinedPool =
    result && result.kind !== "error"
      ? utils.pools.list.getData()?.find((p) => p.id === result.poolId)
      : undefined;
  const accountName = joinedPool
    ? accountTitle(joinedPool.account)
    : "the account";

  return (
    <form
      onSubmit={(e) => void submit(e)}
      className="card flex flex-col gap-4 p-5"
      noValidate
    >
      <div className="flex flex-col gap-1">
        <h2 id="join-title" className="text-lg font-semibold tracking-tight">
          Join with a code
        </h2>
        <p className="text-sm text-ink-2">
          Someone who uses the same Claude account can give you a code.
        </p>
      </div>
      <div className="flex flex-col gap-1.5">
        <label htmlFor={inputId} className="text-sm font-medium">
          Share code
        </label>
        <input
          id={inputId}
          name="code"
          value={code}
          onChange={(e) => {
            setCode(e.target.value);
            if (result?.kind === "error") setResult(null);
          }}
          placeholder="XXXX-XXXX-XXXX"
          autoComplete="off"
          autoCapitalize="characters"
          spellCheck={false}
          maxLength={32}
          aria-describedby={hintId}
          aria-invalid={result?.kind === "error" || undefined}
          className="field font-mono text-base tracking-widest uppercase"
        />
        <p id={hintId} className="text-xs text-ink-3">
          12 letters and digits. Dashes, spaces and case don&apos;t matter.
        </p>
      </div>
      <Callout tone="warning" title="Before you join">
        {SHARING_WARNING}
      </Callout>
      <button
        type="submit"
        className="btn btn-primary"
        disabled={join.isPending || code.trim().length === 0}
      >
        {join.isPending ? "Joining…" : "Join pool"}
      </button>

      <div aria-live="polite">
        {result?.kind === "joined" ? (
          <div
            role="status"
            className="flex flex-col gap-1 rounded-lg border border-line bg-surface-2 p-3 text-sm"
          >
            <p className="font-medium text-good-ink">Joined.</p>
            <p className="text-ink-2">
              You now share {accountName} with{" "}
              {joinedPool
                ? plural(
                    joinedPool.members.length - 1,
                    "other person",
                    "other people",
                  )
                : "the pool"}
              .{" "}
              <Link href={`/accounts/${result.accountKey}`} className="link">
                Open the account
              </Link>
            </p>
          </div>
        ) : result?.kind === "already" ? (
          <div
            role="status"
            className="flex flex-col gap-1 rounded-lg border border-line bg-surface-2 p-3 text-sm"
          >
            <p className="font-medium">You&apos;re already in this pool.</p>
            <p className="text-ink-2">
              Nothing changed.{" "}
              <Link href={`/accounts/${result.accountKey}`} className="link">
                Open {accountName}
              </Link>
            </p>
          </div>
        ) : null}
      </div>
      {result?.kind === "error" ? (
        <p
          role="alert"
          className="rounded-lg border border-critical/40 bg-critical-soft p-3 text-sm text-critical-ink"
        >
          {result.message}
        </p>
      ) : null}
    </form>
  );
}

const NOT_A_CODE = `That doesn't look like a code. A code is 12 letters and digits, like ${POOL_CODE_EXAMPLE}.`;

function joinErrorMessage(error: unknown): string {
  switch (errorCode(error)) {
    case "NOT_FOUND":
      // Unknown, expired and revoked codes get the same answer from the server.
      return "That code doesn't work. Check it for typos; if it's right, it has expired (codes last 7 days) or was revoked, so ask for a new one.";
    case "TOO_MANY_REQUESTS":
      return "Too many codes that didn't work. Wait an hour, then try again.";
    case "BAD_REQUEST":
      return NOT_A_CODE;
    default:
      return describeError(error).message;
  }
}

// ---------------------------------------------------------------------------------------------
// Pools you joined

function JoinedPools() {
  const [pools] = api.pools.list.useSuspenseQuery();
  const joined = pools.filter((p) => p.role === "member");
  if (joined.length === 0) {
    return (
      <p className="card p-5 text-sm text-ink-2">
        You haven&apos;t joined anyone&apos;s pool.
      </p>
    );
  }
  return (
    <ul className="grid gap-4 md:grid-cols-2">
      {joined.map((pool) => (
        <li key={pool.id}>
          <JoinedPoolCard pool={pool} />
        </li>
      ))}
    </ul>
  );
}

function JoinedPoolCard({ pool }: { pool: Pool }) {
  const invalidate = useInvalidatePools();
  const leave = api.pools.leave.useMutation({ onSuccess: invalidate });
  const subtitle = accountSubtitle(pool.account);
  return (
    <article className="card flex h-full flex-col gap-4 p-5">
      <header className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="font-semibold break-words">
            <Link
              href={`/accounts/${pool.accountKey}`}
              className="hover:underline"
            >
              {accountTitle(pool.account)}
            </Link>
          </h3>
          {subtitle ? (
            <p className="text-sm break-words text-ink-2">{subtitle}</p>
          ) : null}
          <p className="text-sm break-all text-ink-2">
            Shared by {pool.createdBy.displayName}
            {pool.createdBy.name ? ` (${pool.createdBy.name})` : null}
          </p>
        </div>
        {pool.account.plan ? <Badge>{pool.account.plan}</Badge> : null}
      </header>
      <MemberList pool={pool} />
      <div className="mt-auto flex flex-col gap-2 border-t border-line pt-4">
        <ConfirmAction
          trigger="Leave pool"
          question={
            <p>
              <span className="font-medium">Leave this pool?</span> You stop
              seeing the other members&apos; sessions on this account, and they
              stop seeing yours. You&apos;d need a code to join again.
            </p>
          }
          confirmLabel="Leave pool"
          pending={leave.isPending}
          onConfirm={() => leave.mutateAsync({ poolId: pool.id })}
        />
        <MutationError error={leave.error} what="leave the pool" />
      </div>
    </article>
  );
}

function MutationError({ error, what }: { error: unknown; what: string }) {
  if (!error) return null;
  return (
    <p role="alert" className="text-sm text-critical-ink">
      Couldn&apos;t {what}: {describeError(error).message}
    </p>
  );
}
