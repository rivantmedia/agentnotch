"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

import {
  DELETE_DATA_CONFIRMATION,
  REMOVE_SUMMARIES_CONFIRMATION,
} from "~/lib/data-controls";
import { plural } from "~/lib/format";
import { describeError } from "~/trpc/errors";
import { api } from "~/trpc/react";

import { TypedConfirmAction } from "../_components/typed-confirm-action";
import { Callout } from "../_components/ui";
import { SYNC_AGAIN_NOTE } from "./copy";

/** Removing the viewer's summaries, or everything they synced, each behind a typed phrase. */
export function DataControls() {
  const router = useRouter();
  const utils = api.useUtils();
  const [done, setDone] = useState<string | null>(null);
  const refresh = async () => {
    // Every page's data may have changed; the Macs list on this page is rendered on the server.
    await utils.invalidate();
    router.refresh();
  };
  const removeSummaries = api.myData.removeSummaries.useMutation({
    onMutate: () => setDone(null),
    onSuccess: async ({ sessions }) => {
      await refresh();
      setDone(
        sessions === 0
          ? "There were no summaries to remove."
          : `Removed the summaries of ${plural(sessions, "session", "sessions")}.`,
      );
    },
  });
  const deleteAll = api.myData.deleteAll.useMutation({
    onMutate: () => setDone(null),
    onSuccess: async (deleted) => {
      await refresh();
      setDone(
        `Deleted ${plural(deleted.sessions, "session", "sessions")}, ${plural(deleted.usageReadings, "usage reading", "usage readings")} and ${plural(deleted.devices, "Mac", "Macs")}.`,
      );
    },
  });

  return (
    <div className="flex flex-col gap-4">
      <article className="card flex flex-col gap-3 p-5">
        <h3 className="font-semibold">Remove my summaries</h3>
        <p className="text-sm text-ink-2">
          Clears the summary of every session your Macs synced: its text, the
          model that wrote it and when. Titles, times, tokens and costs stay.
          Pool members stop seeing your summaries too. A session a pool member
          also synced still shows their summary of it, if they have one.
        </p>
        <TypedConfirmAction
          trigger="Remove my summaries"
          phrase={REMOVE_SUMMARIES_CONFIRMATION}
          confirmLabel="Remove summaries"
          pending={removeSummaries.isPending}
          onConfirm={() =>
            removeSummaries.mutateAsync({
              confirm: REMOVE_SUMMARIES_CONFIRMATION,
            })
          }
        >
          <p>
            <span className="font-medium text-ink">
              Remove the summaries of all your sessions?
            </span>{" "}
            This can&apos;t be undone.
          </p>
          <p>
            A Mac sends a summary again only with a session that changes later
            while &ldquo;Summarise finished sessions with Claude&rdquo; is on in
            the app. Turn it off there (Settings &gt; Claude Code &gt; Cloud) to
            keep them away.
          </p>
        </TypedConfirmAction>
        <MutationError error={removeSummaries.error} what="remove them" />
      </article>

      <article className="card flex flex-col gap-3 p-5">
        <h3 className="font-semibold">Delete all my synced data</h3>
        <p className="text-sm text-ink-2">
          Deletes everything your Macs synced: your sessions and their
          summaries, projects, usage readings, the Claude accounts they reported
          and the Macs themselves. Pools you created are deleted, so their
          members stop seeing each other&apos;s sessions through them, and you
          leave the pools you joined. You stay signed in.
        </p>
        <Callout tone="warning" title="Turn sync off in the app first">
          {SYNC_AGAIN_NOTE}
        </Callout>
        <TypedConfirmAction
          trigger="Delete all my synced data"
          phrase={DELETE_DATA_CONFIRMATION}
          confirmLabel="Delete everything"
          pending={deleteAll.isPending}
          onConfirm={() =>
            deleteAll.mutateAsync({ confirm: DELETE_DATA_CONFIRMATION })
          }
        >
          <p>
            <span className="font-medium text-ink">
              Delete all your synced data?
            </span>{" "}
            This can&apos;t be undone, and the people in pools you created lose
            them too.
          </p>
        </TypedConfirmAction>
        <MutationError error={deleteAll.error} what="delete it" />
      </article>

      <p aria-live="polite" className="text-sm font-medium text-good-ink">
        {done}
      </p>
    </div>
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
