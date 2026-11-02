import { IconTrendingDown, IconTrendingUp } from "@tabler/icons-react"

import { Badge } from "@/components/ui/badge"
import {
  Card,
  CardAction,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

export type SectionCard = {
  label: string
  value: string
  /** Change against the previous window, in percent. */
  change?: number
  /** Whether a rise is bad (errors) rather than neutral (traffic). */
  riseIsBad?: boolean
  headline: string
  note: string
}

/** dashboard-01's section cards, fed from props instead of sample data. */
export function SectionCards({ items }: { items: SectionCard[] }) {
  return (
    <div className="grid grid-cols-1 gap-4 px-4 *:data-[slot=card]:bg-gradient-to-t *:data-[slot=card]:from-primary/5 *:data-[slot=card]:to-card *:data-[slot=card]:shadow-xs lg:px-6 @xl/main:grid-cols-2 @5xl/main:grid-cols-5 dark:*:data-[slot=card]:bg-card">
      {items.map((item) => {
        const up = (item.change ?? 0) >= 0
        const Trend = up ? IconTrendingUp : IconTrendingDown
        return (
          <Card key={item.label} className="@container/card" data-kpi={item.label}>
            <CardHeader>
              <CardDescription>{item.label}</CardDescription>
              <CardTitle className="text-2xl font-semibold tabular-nums @[250px]/card:text-3xl">
                {item.value}
              </CardTitle>
              {item.change !== undefined && Number.isFinite(item.change) && (
                <CardAction>
                  <Badge
                    variant="outline"
                    className={
                      item.riseIsBad && up && item.change > 0
                        ? "text-destructive"
                        : undefined
                    }
                  >
                    <Trend />
                    {up ? "+" : ""}
                    {item.change.toFixed(1)}%
                  </Badge>
                </CardAction>
              )}
            </CardHeader>
            <CardFooter className="flex-col items-start gap-1.5 text-sm">
              <div className="line-clamp-1 flex gap-2 font-medium">
                {item.headline}
              </div>
              <div className="text-muted-foreground">{item.note}</div>
            </CardFooter>
          </Card>
        )
      })}
    </div>
  )
}
