import { LoadingBlock, Skeleton } from "../_components/ui";

export default function UsageLoading() {
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-8 px-4 py-8 sm:py-10">
      <LoadingBlock label="Loading your usage…" className="flex flex-col gap-8">
        <div className="flex flex-col gap-2">
          <Skeleton className="h-8 w-32" />
          <Skeleton className="h-4 w-96 max-w-full" />
        </div>
        <div className="flex flex-wrap gap-2" aria-hidden="true">
          <Skeleton className="h-8 w-56 rounded-lg" />
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-8 w-32 rounded-full" />
          ))}
        </div>
        <div
          className="card grid grid-cols-2 gap-5 p-5 sm:grid-cols-4"
          aria-hidden="true"
        >
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-12" />
          ))}
        </div>
        <div className="grid gap-4 md:grid-cols-2" aria-hidden="true">
          {[0, 1].map((i) => (
            <div key={i} className="card flex flex-col gap-3 p-5">
              <Skeleton className="h-4 w-24" />
              <Skeleton className="h-40 w-full" />
            </div>
          ))}
        </div>
      </LoadingBlock>
    </div>
  );
}
