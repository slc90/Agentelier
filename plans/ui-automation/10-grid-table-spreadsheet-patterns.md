# 接入 Grid、Table 与 Spreadsheet Pattern

## 目标与产出

提供网格、表格和电子表格的行列信息、单元格定位、标题与公式/值读取能力。

## 设计

接入 Grid、GridItem、Table、TableItem、Spreadsheet 和 SpreadsheetItem。

- Grid：行数、列数和指定单元格。
- GridItem：单元格行列索引、所属 Grid 和跨度。
- Table：行列结构和行/列标题。
- TableItem：单元格所属行/列标题。
- Spreadsheet：电子表格专有状态。
- SpreadsheetItem：单元格公式和值等 Provider 暴露信息。

所有访问按行列范围和单元格数量设预算。不要默认枚举整个大型网格。Notepad 和 File Explorer 未必提供 Spreadsheet Pattern，应用是否支持以实际查询为准。

## Rhai API

提供 `uia_grid_info`、`uia_grid_cell`、`uia_grid_item_info`、`uia_table_info`、`uia_table_headers`、`uia_table_item_info`、`uia_spreadsheet_info` 和 `uia_spreadsheet_item_info`。

## 范围

包含上述六类 Pattern 的可用性、元数据和有界单元格访问。不增加 Excel 作为验收软件。

## 验收

- Pattern 支持情况准确。
- Grid/Table 能读取行列元数据。
- GridItem/TableItem 能读取单元格位置和标题信息（Provider 支持时）。
- Spreadsheet/SpreadsheetItem 能读取 Provider 实际暴露的信息。
- 大型表格读取有界。
- 不支持 Pattern 返回明确结果。
- 在 File Explorer 详细信息视图检查 Grid/GridItem/Table/TableItem 的实际支持情况；Notepad 作为负向/不支持情况记录。
