import { LoadingBlock, Skeleton } from "../_components/ui";

export default function PoolsLoading() {
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-10 px-4 py-8 sm:py-10">
      <LoadingBlock label="Loading pools…" className="flex flex-col gap-8">
        <div className="flex flex-col gap-2">
          <Skeleton className="h-8 w-32" />
          <Skeleton className="h-4 w-96 max-w-full" />
        </div>
        <div
          className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_22rem]"
          aria-hidden="true"
        >
          <div className="card flex flex-col gap-3 p-5">
            <Skeleton className="h-4 w-44" />
            <Skeleton className="h-9 w-40" />
          </div>
          <div className="card flex flex-col gap-3 p-5">
            <Skeleton className="h-4 w-32" />
            <Skeleton className="h-9 w-full" />
          </div>
        </div>
      </LoadingBlock>
    </div>
  );
}
