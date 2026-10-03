# ADR-548 — точный ранний отсев запросов крыши

03.10.2026, HEAD до изменения `5bf8823`, продолжение
[141](141-parallel-combustion-contacts.md), WBPERF-005/011.

## Участок и принятый вариант

Свежий CPU-профиль world-battle AI против AI, 30×30, 3600 тиков,
physics/navigation=8 подтверждает прежний порядок дорогих фаз: motion,
debris, rubble. В navigation worker profile работа присутствует на всех
восьми OS threads; неравномерные длинные задания отдельных юнитов остаются.
Низкий общий CPU% сам по себе не означает, что этот пул не используется.
Диагностический запуск — `out/fix548/before-workers.log` и
`before-workers-ticks.csv`;
его стоимость с наблюдателями не используется как ordinary A/B.

В параллельном выборе кандидатов Settled корпус решает, несёт ли он
обломок. `Hull::lowest` начинает искать воксели с `floor(pos.y)`;
первая возможная верхняя грань — целая высота `floor(pos.y)+1`.
Если опорная проба `bottom+1/2` ниже этой грани, прежняя проверка крыши
обязательно вернёт false. Теперь `Hull::carries` возвращает его до
проверки повёрнутого footprint и запроса вокселей.

Отсев действует только при наличии `Solid`, следует прежнему округлению
base к i32 и сравнению высот в Fx. Корпус без вокселей и запросы на
высоте возможной крыши сохраняют прежний путь. Округление отрицательных
координат — арифметический shift, как в `lowest`. Нижние сегменты не
вводят нового правила: эталон тоже не искал крышу ниже своей base.

Нет новых пулов, saved state, кешей, зависимостей, изменений сил/опор или
визуального качества. Ускоряется работа существующих parallel candidate
jobs, а не добавляется очередной барьер. Использование CPU равномернее
само по себе этим изменением не доказано.

## Отменённые эксперименты

Перед этим проверены два варианта индекса; оба убраны из рабочего кода.

1. Word-range membership и перечисление выбранных Y bits вместо
   поклеточного построения contact lists. Полный тик в одной CPU-паре
   12,970→13,433 мс (+3,57%), cells mean 213,25→283,67 мкс.
   Одна пара не изолирует алгоритм от OS noise, но выигрыша не показала.
   559 тестов проходили, состояние боя совпадало. Frozen exe и raw —
   `rejected-word-cells-bench.exe`, `rejected-word-cells-*`,
   `rejected-before-*` в `out/fix548`.
2. Однократный scatter source indices в восемь переиспользуемых X lists
   плюс word-range membership при выборе кандидатов. Пары полного тика:
   13,157→13,031 и 12,965→12,976 мс. Повтор не подтвердил ускорение;
   cells mean 231,42→226,92 и 214,42→229,67 мкс. Дополнительные буферы и
   серийный scatter не оставлены. Проверки contact order при 1/2/4/8
   потоках проходили. Код/binary/summary и четыре пары log/CSV —
   `rejected-stripe-inputs.rs`, `rejected-stripe-inputs-bench.exe`,
   `rejected-stripe-summary.json`, `rejected-stripe-*`.

Отмена не означает, что эти схемы проигрывают на любом наборе данных.
Здесь не выполнено условие измеримого ускорения штатного большого боя.

## Проверки принятого варианта

Полный release sim-core на финальном коде: **559 passed, 4 ignored**.
Новая регрессия сравнивает `carries` с прежним `lowest` при отрицательных,
целых и дробных bases, четырёх heading, отверстиях, точках внутри и вне
footprint, низких и высоких пробах, с `Solid` и без него. Atomic counter
в тестовом Voxels подтверждает ноль запросов при отсеве и реальное
чтение геометрии для допустимой крыши. Он не входит в production.
Прежние support/fragment/contact/pool/replay tests проходят.
Clippy release lib проходит без новых warnings; остаются прежние
voxel/nav type complexity и sim redundant closure. Rustfmt/diff check
проходят. Пять старых архитектурных падений, воспроизведённых в ADR-547,
не исправлены этим шагом; общий CI не объявляется зелёным.

## CPU-пары

Frozen процессы запускаются последовательно, один за раз;
`RAYON_NUM_THREADS=8 --world --ai-vs-ai --ticks 3600 --tick-csv ...`.
Порядок принятого варианта: before → after → before-repeat → after-repeat.
Каждый из 3600 тиков имеет те же searching/moving/blocked/fragments/
flying/chunks/felling; fired/damage/alive/bases и финальный hash
`bcc1f58c226d1721` совпадают у всех четырёх запусков.

| Run, out/fix548 | Tick mean, ms | Tick p99, ms | Debris mean, ms | Settled candidates mean, ms |
|---|---:|---:|---:|---:|
| before | 12,9612 | 21,505 | 2,939 | 0,5675 |
| after | 12,8089 | 20,958 | 2,872 | 0,4426 |
| before-repeat | 12,9547 | 21,361 | 2,954 | 0,5581 |
| after-repeat | 12,8624 | 21,567 | 2,912 | 0,4438 |

Измеренное сокращение candidates — 20,5…22,0%, debris — 1,4…2,3%,
полного тика — 0,7…1,2%. Это небольшой выигрыш, не решение критического
пути кадра. Max тик 60,597→50,513 и 48,779→65,750 мс; второй p99 выше,
стабильного исправления длинных пауз нет. `bench-summary.json` сверяет
state/counters и сохраняет абсолютные значения; CPU CSV не являются FPS.

## Обычный полигон

Одна последовательная пара по 120 с, normal build без render-tasks и
diagnostic collectors, без сборок/тестов/правок/тяжёлого анализа во время
запуска. Одинаковые RON, observed 1600×900/AutoNoVsync, камера и эффекты;
compute=4, physics/navigation=8. Native и offline gates проходят;
strict comparison eligible, по 3600 шагов / ~0,9995×, hash одинаковый.

| Run, out/fix548 | Mean frame, ms | 1000/mean, FPS | Frame p99, ms | Max frame, ms | Core/step mean, ms |
|---|---:|---:|---:|---:|---:|
| before-native-120 | 12,8679 | 77,713 | 33,3002 | 66,1137 | 14,2906 |
| after-native-120 | 12,5702 | 79,553 | 32,7322 | 77,1704 | 14,1040 |

Observed FPS +2,37%, core/step −1,31%. Одна пара не доказывает
статистическую значимость или выигрыш на любом хосте/составе боя.
165 FPS не достигнуты; maximum after выше before, редкие stalls не
исправлены. В after row8666/tick3052 core 56,799 мс, Update slice
70,682 мс, следующий интервал 77,170. Row6218/tick1606 core52,998.
Подробных worker phases/OS stacks в ordinary run нет: причина этих
пауз не установлена, их нельзя объявить lock wait/GPU/new branch.

`actions-20`: штатные сценарные reset generations 1…4, 456 шагов;
native/offline gates проходят, hash `b9be7da46fa8f9b7`. Mean frame
2,701 / p99 8,220 / max44,866 мс. Это отдельный smoke с паузами и
сбросами, не FPS-приёмка непрерывного world-battle и не A/B хеш другого
actions-запуска с иным числом шагов.
Release sim-cli seed42/1000ticks/5000units/threads1:
hash `66041a9f60189841`. Playable release build проходит.

Frozen SHA256:

- ordinary before `422a9a556f22141e232acdf33dafc2afc10f3b12675e11eabe2c1d2202d714de`;
- ordinary after `4021acbe2ae9d488287049c3b3f0daa33c474d00915b8539fc7065daa3168c0b`;
- benchmark before `847a37071043e0896299cda494bea19a19d53a6aa86897452395d199cb721da2`;
- benchmark after `5bff243c980dadecfe1368ca3126f39877b6c952e99f7fb3f1029fd647d745ac`.

Оба варианта содержат одинаковую незакоммиченную ADR-523 работу;
это не чистые Git snapshots. HEAD/SourceAtLaunch в manifest не
доказывают build source identity; исполняемый файл идентифицирует SHA.
Все raw CSV/manifests/logs/analysis reports находятся в названных папках.

WBPERF-002/027/001, motion/routes и mesh queues остаются следующими
приоритетами. Равномерную загрузку CPU и причины stalls нельзя выводить
из одного parent wall; нужны child/OS evidence и независимый sim/render.
