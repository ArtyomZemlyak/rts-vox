# ADR-546 — параллельные позы юнитов и проверка compute pool

Дата: 03.10.2026. Продолжение [ADR-545](139-render-prepare-system-calls.md),
WBPERF-001/013/027. Изменены только рендер поз и управление экспериментом;
авторитетная симуляция, баланс, видимость и качество изображения прежние.

## Причина и изменение

ADR-545 нашёл фактический `collect_meshes_for_gpu_building` до 20,740 мс
внутри prepare 23,741 мс. Его nested producers/collection ещё не имеют
child attribution; эти значения не доказывают нехватку потоков или lock wait.

Pinned Bevy 0.19.1 mesh extraction читает Changed<GlobalTransform> среди
остальных фильтров. `sync_simple_transforms` переносит Changed<Transform>
в GlobalTransform. Наш `moving::animate` каждый кадр безусловно записывал
Transform всех Witness, включая неподвижные корпуса, базы и турели.
Повторял вычисление hull/turret rotation для каждого меша, заново создавал
векторы поз и ног. Это конкретный источник лишней инвалидизации.

Теперь pose snapshot строится один раз на юнит. Буферы кадров и ног
переиспользуются; hull/turret quaternion вычисляется один раз. Применение
снимка к независимым Witness выполняет `Query::par_iter_mut` на постоянном
Bevy ComputeTaskPool. `set_if_neq` сохраняет change tick неизменного
Transform. Minimal callers без инициализированного пула имеют serial fallback.
Симуляция читается неизменяемо, редукций/взаимных записей юнитов нет.
Порядок вращений, масштаб, наклон корпуса, интерполяция башни и восьми ног
паука сохранены. Больше worker CPU time этой системой не измерено.

Для независимой проверки пула добавлены `--compute-workers 1…16` и
`profile-range.ps1 -ComputeWorkers N`; 0 в скрипте означает default.
TaskPoolPlugin фиксирует только compute min/max. IO, async, physics и
navigation настройки не меняются. Без override остаётся стандартная
политика Bevy. На этом хосте фактически compute=4, physics/navigation=8/8;
Win32 сообщает восемь logical processors. Это не вывод о полной аппаратной
конфигурации или ограничениях affinity.

Manifest, runtime identity и отчёт содержат запрошенное/фактическое число.
При override требуется ровно одна CLI identity и один actual pool count,
совпадающие с запросом: старый exe, молча игнорирующий аргумент, invalid.
Смена requested или actual compute count отклоняет строгую пару;
legacy absent requested нормализуется к 0, absent actual остаётся unknown.
Сравнение вариантов пула ниже — отдельный эксперимент, а не eligible A/B.

## Проверки

379 проверок: 130 render library, 106 blast, 94 Python и 49 PowerShell;
ещё четыре ручных теста ignored. Release tests, clippy и build проходят;
новых clippy warnings нет, существующие type_complexity/redundant_closure
не изменены. Новых зависимостей, unsafe или смены версий нет.

Два новых теста запускают настоящую animate-систему с MinimalPlugins и
TransformPlugin для всех семи типов. Результат точно совпадает с прежними
serial formulas, включая наклон, turret angular wrap, сегменты, восемь ног,
интерполяцию, смерть и reset. Snapshot симуляции неизменен. PostUpdate
observer после TransformSystems::Propagate доказывает: первый кадр имеет
Changed local/global; следующий неподвижный — ноль; движение одного юнита
меняет только его Witness. Это наблюдение до очистки trackers, а не
пустой Query после App::update. Проверяется сохранение ёмкости pose buffer.

Повторный запуск существующего sim-cli: seed 42, 1000 ticks, 5000 units,
threads=1, hash `66041a9f60189841`. Симуляционный код этой работой не менялся.

## Нативные замеры

Обычные release exe без render-tasks feature; нет diagnostic observers,
GPU timestamps, core traces или OS sampling. Один процесс за раз, без
сборок/тестов/правок и тяжёлого анализа во время замера. World-battle:
30×30, одинаковые входные RON, камера/эффекты, 1600×900, AutoNoVsync,
TargetFps=165. Gate проверяет observed context, входы, скорость симуляции
и hash. Eligibility означает допустимость evidence, не выполнение FPS budget.

Frozen before — `out/fix546/before.exe`, SHA256
`a4795b1d46466520a95d6ffd68990b1fcd6f381aeaf9aa3a3e9474983636403a`.
Frozen after — `out/fix546/after.exe`, SHA256
`7c9080ce252c13fe254003882ec51026e59728233adfe290e520996a5876e9c4`.
HEAD на запуске `7448b5c`; оба exe включают одинаковую незакоммиченную
ADR-523 работу selection/laser. Это не clean Git snapshots; HEAD в manifest
не доказывает source identity сборки. SHA фиксирует измеренный binary.

| Run, out/fix546 | Actual compute | Frame mean, ms | 1000/mean, FPS | p99, ms | Max, ms | Core mean/step, ms |
|---|---:|---:|---:|---:|---:|---:|
| before-60 | 4 | 9,9274 | 100,731 | 30,0403 | 64,8250 | 11,9130 |
| after-default-60 | 4 | 9,5617 | 104,583 | 29,6802 | 65,8606 | 11,8803 |
| after-compute8-60 | 8 | 9,5054 | 105,204 | 29,1376 | 66,3618 | 11,8370 |
| before-repeat-60 | 4 | 9,4451 | 105,875 | 29,7254 | 66,3365 | 11,7489 |
| after-repeat-60 | 4 | 9,4061 | 106,314 | 29,3427 | 66,3248 | 11,7665 |
| after-default-120 | 4 | 13,3160 | 75,098 | 34,1559 | 65,2223 | 14,8058 |
| before-120 | 4 | 13,4284 | 74,469 | 33,8486 | 65,0239 | 14,6629 |

Все минутные бои: 1800 шагов, 0,9994…0,9996×, одинаковый hash
`bdfb97cef98d13e2`; оба двухминутных — 3600 / ~0,9995×,
hash `bcc1f58c226d1721`. Обычные пары 60/60, repeat 60/60 и 120/120
проходят native/offline comparison gates. Порядок запусков указан в таблице:
длинная пара выполнена after-before. Контрольная старая сборка повторно
запущена, а не заменена прошлыми диагностическими результатами ADR-545.

Первая пара: observed FPS +3,82%; повтор: +0,41%; длинная: +0,84%.
Разброс before минутных runs около 5%, больше основного выигрыша.
Устойчивый крупный FPS прирост, статистическая значимость и отсутствие
любой tail regression этими runs не доказаны. Длинный after p99 на
0,307 мс выше before, core mean/step примерно на 0,97% выше; симуляция
не ускорялась. Нельзя приписать изменение общего FPS одному pose apply
без отдельного измерения этой системы.

Compute=8 дал +0,59% относительно первого after default, максимум чуть
выше. Вариант валиден как самостоятельный run, но strict pair отклонена
по requested/actual compute count. Размер пула по умолчанию оставлен
прежним: убедительного выигрыша от удвоения нет.

`actions-20`: 455 сценарных шагов, generations 1…4, native/offline gates
проходят; mean 2,807 / p99 8,595 / max 45,807 мс. Это reset/placement
smoke со сценарными паузами, не приёмка realtime world-battle или 165 FPS.
JSON/Markdown, manifest, stderr и frame CSV остаются в указанных out paths.

## Оставшееся

165 FPS не достигнуты; максимум ~66 мс и рост mean длинного боя сохраняются.
Неподвижные Witness перестали создавать ложные Transform updates, но
changed/new/removed mesh queue counts и child worker CPU/wait остаются
неизмеренными. Следующий шаг WBPERF-001/027 — наблюдать эту нагрузку и
сопоставить реальные producer/consumer jobs со source frame; затем
WBPERF-002 — последовательный тик на критическом пути кадра. Не объявлять
низкий общий CPU% доказательством недостаточного количества compute workers.
