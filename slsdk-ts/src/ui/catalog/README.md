# superlive A2UI catalog

`superlive.catalog.json` 是 superlive 共享组件设施的 A2UI v0.9.1 catalog——一份可整份下发的 JSON Schema 文件（catalogId `superlive`），公共类型引用指向官方 `common_types.json`。`types.ts` 是其 TS 伴随类型，JSON 为权威源。

## 映射规则（宿主 props → catalog schema）

1. 值 props → properties：string / number / boolean 直录；需要动态数据的值用 common_types 的 Dynamic*（字面量 | DataBinding | FunctionCall）。
2. 回调 props → action：统一映射为组件级 Action 属性，命名 `<回调名去 on 前缀>Action`（如 `onFilterChange` → `filterChangeAction`、`onRetry` → `retryAction`）；事件名与 context 键进描述面，宿主实现不进。
3. ReactNode props → 组件引用：单节点 → `child: $ref ComponentId`，节点列表 → `children: $ref ChildList`（EmptyState 的 action 节点同样落为 `child`）；className / id / name / autoComplete 等宿主细节不进描述面。
4. 泛型与渲染回调降维：DataTable 的泛型 `T` 与 render 回调不进描述面；纯数据 schema 直录为复合 properties（FormDialog.formSchema ← FormFieldSchema/FormSchema，QueryListToolbar.querySchema ← QuerySchema/QueryFieldSpec，Select.options ← SelectOption）。
5. 注入表不进描述面：references 注入表、fixedExtra 业务固定条件等运行时注入不进 catalog，schema 只声明键（如 reference.entity），数据源由消费方运行时注入。

## 对齐基准

A2UI specification v0.9.1 快照（common_types.json / protocol.md / basic_catalog.json）；catalog 结构沿用官方 basic catalog 形态（Apache-2.0）。
