import { LoadingBlock, Skeleton } from "../_components/ui";

/** Shown while GitHub answers, when its answer isn't cached yet. */
export default function DownloadLoading() {
  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-10 px-4 py-8 sm:py-12">
      <LoadingBlock
        label="Loading the latest release…"
        className="flex flex-col gap-10"
      >
        <div className="flex flex-col gap-2" aria-hidden="true">
          <Skeleton className="h-8 w-72 max-w-full" />
          <Skeleton className="h-5 w-56 max-w-full" />
        </div>
        <div className="grid gap-4 md:grid-cols-3" aria-hidden="true">
          {[0, 1, 2].map((i) => (
            <div key={i} className="card flex flex-col gap-3 p-5">
              <Skeleton className="h-5 w-20" />
              <Skeleton className="h-4 w-48 max-w-full" />
              <Skeleton className="h-9 w-full" />
            </div>
          ))}
        </div>
      </LoadingBlock>
    </div>
  );
}
