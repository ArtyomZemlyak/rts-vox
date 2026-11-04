# 441 — Слияние selected и cell partition `Settled`: отклонено

2 ноября 2026. Артефакты: `out/optimization-441/`.

## Гипотеза

`selected` занимает большой временный массив и читается при shape collection и
cell partition. Прототип убрал его: candidates сразу собирали bounds в cell
requests и shapes, после чего `build_requests` создавал прежние linked cells.
Старый selected-builder сохранялся для тестов.

## Проверки

- `settled_candidates_keep_cell_lists_in_fragment_order_across_workers` прошёл:
  порядок каждого cell list на1/4 workers совпал.
- Wide links, irregular shape и lab snapshot/restore прошли.
- Release-сборка прошла.
- Все36 нетайминговых полей каждого тика совпали; hash `ef96977a5b55489b` прежний.

## Измерения

Baseline `out/optimization-439/baseline-825.exe`, SHA-256
`ccfe075969c4c853a2a4b21398f7b6b9bd1aa0511cef9a053dc87beaed0cd991`.
World AI-vs-AI,1600 тиков,physics4/navigation8/Rayon4, порядок «после/до».

| Показатель | Baseline | После | Изменение |
|---|---:|---:|---:|
|Полный mean|31,959 мс|32,972 мс|+3,17%|
|Позднее окно1201–1600|45,561 мс|47,631 мс|+4,54%|
|`debris`|13,680 мс|14,712 мс|+7,54%|
|Candidates+cells, позднее окно|7,385 мс|9,334 мс|+26,40%|

`cells` стал дешевле, но candidates теперь включает укрупнённые requests и shape
collection. Их совместная стоимость выросла. Копирование полных bounds в каждый
request и chunked scratch оказались дороже устранённого массива selected.

## Решение

Прототип отклонён, рабочий `burst.rs` возвращён к ADR-825. ADR не создаётся.
Локальное ускорение одной подфазы недостаточно, если увеличивает суммарную работу.
Следующие изменения `Settled` должны сохранять компактные запросы и проверять
сумму candidates+cells, а не только конечный fill.
