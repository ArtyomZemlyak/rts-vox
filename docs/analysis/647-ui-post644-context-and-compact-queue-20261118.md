# 647 — контекст каталога и компактная очередь после646

Проверен Antigravity06 SHA256
`5bb77d5cef9f60dee96551feb58813de80a3132db8ccd50248103d37e920f6e8`.
Review выполнен на текущей незакоммиченной работе646, а не на старом644.
Текущий compile source:
`7ea0214a11bb775760d68ab4eedef5d4eb86d44c2b960a88ba6e9080e577e131`.
Development binary:
`4ab511f8dc7d9425ec78d70bedee92ceb42cf407bc0a787a7b76aba2d05726da`.

## Что уже было и что исправлено

| Часть handoff | Проверка текущего кода / действие |
| --- | --- |
| projection=None скрывает Fold/Tools | Корневой whitelist уже есть; сохранён без повторной правки |
| неподвижный скрытый Scan пишет Node | `if state.motion.work` уже есть; сохранён |
| orphan Filters/Row при боевом выборе | Исправлено через один владелец видимости |
| верхняя очередь540p слишком широка | Исправлена ширина и устранена холостая запись её root |
| order71 prepared/idle/AMBER | Эквивалент644 подтверждён; повторный перенос не нужен |

Старый `git apply --check`06 отказал на уже реализованном последнем hunk.
Diff не накладывался вслепую. `ui_motion` prepared/Duration и AMBER theme
побайтово сохранены, как и механика646/645. Изменены только пять UI-файлов:
`construction.rs`, `construction/tests.rs`, `construction/production_hud.rs`,
`spatial_ui.rs`, `strategic_icons.rs`. Аудит и patch относительно сохранённого646:
`out/ui647/source-scope-audit.json`, `out/ui647/implementation.patch`.

`Catalog::is_hidden()` учитывает Hidden context и актуальный результат
refresh о доступности каталога в меню/lab. Только `spatial_ui::update`
назначает display дочерним Row/Filters после разрешения проекции:
боевой Tank/Spider и отсутствие строителя скрывают обе части;
закрытый компактный drawer тоже скрывает обе. На desktop при открытой
context shelf нижние фильтры остаются на месте, Row не дублирует рецепты.

Предложенное06 безусловное открытие обеих частей в construction refresh
не перенесено: оно давало бы Flex→None каждый кадр перед spatial update.
Новые дополнительные Query/tuple-аргументы refresh поэтому не нужны.
Видимость Root сохраняет прежнего владельца `sync_visibility`.

Очередь использует `(viewport_width*0.28).clamp(256,360)`:
268,8px на960×540,358,4px на1280×720,360px на1080p/4K.
Top/right не изменяются при выборе. Её root получает Node-запись лишь
при изменении display/width. Команды, производство, pooling, материалы,
палитра и SIM не изменены.

При нативном просмотре4K найдена ещё одна существующая ошибка: значки
юнитов не привязаны к моделям при UiScale2. Тот же кадр на замороженном646
подтвердил её наличие до этого пакета. `strategic_icons::update` не переводил
`Camera::world_to_viewport` в координаты UI. Исправлены и значки, и полосы
стройки: делятся экранная точка и viewport для clipping/LOD; постоянные
отступы остаются в UI-единицах. Цвета, размеры и данные строительства прежние.

## Причинные и focused проверки

На старом runtime две проверки дали0/2: лишние Filters/Row при выборе Tank
и width360 вместо268,8. Исходный receipt и failure log сохранены в
`out/ui647/checks-before`; его путь в stdout первого runner был checks-red.
Первый расширенный probe ошибочно ожидал не создаваемый native builder
`Part::Filter` и получил1/2; это исправление fixture, не runtime-проблема.
Probe сохранён отдельно как `checks-red`; он не засчитан в pass.

Две проверки прошли на промежуточном926dabac и включены в финальный набор.
Настоящие builders создают
каталог, очередь и spatial chrome. Проверены Tank, Spider, deselection,
Factory, возврат из меню, клики по существующим Fold/Restore/Tools и
открытие/закрытие drawer. Размеры960×540,1920×1080,3840×2160 (UiScale2
для4K); открытые Row/Filters не меняют last_changed в покое. Отдельная
очередь проверена на четырёх ширинах с настоящим Produce и стабильным root.
Мышиные Interaction в этих headless проверках дополнены нативным прогоном ниже.

Промежуточный926dabac: `checks-final/test-index.json`, **70/70**, failed/ignored0.
Эти результаты не подменяют проверку финального source. Сохранены настоящие
timed upgrades и motion contracts, реальные production exits при100/35/0,
idle layout, прозрачные промежутки и защита от stale selection.
Явно пропущен только старый дорогой stock-world sweep
`construction::tests::all_stock_scenarios_offer_only_their_live_allied_builders_recipes`.
Он не выполнялся и не считается pass. Старые receipts автора не засчитаны.

Доказательство исходного DPI-дефекта — нативный baseline646 на4K и сравнение
с финальным кадром на тех же входах. Первые headless DPI-red результаты
не считаются причинным доказательством: фикстура без Window позволяла
panel::actions сбрасывать фактический UiScale в1 при ожидаемом2. Исправлены
сама фикстура и утверждение фактического UiScale, без дополнительного
runtime-обхода. Проверка использует настоящие Scene/Camera/Icons и созданный
через SIM строительный план. Самостоятельный исправленный тест прошёл1/1,
включая значок, полосу строительства и возврат масштаба1→2→1; он также
входит в итоговые73. Отказы и ошибка подготовки теста сохранены в checks-dpi-*
и checks-final-dpi, не засчитаны в pass.

Финальный source выше: `checks-final-current/test-index.json` — **73/73**,
failed/ignored0; включая DPI, предыдущие два regressions, motion/input и
реальное производство. `checks-architecture-docs-final-current`:12architecture
и3docs, без skips. Единственный явный skip UI — stock-world sweep выше.
Заключительный docs receipt после обновления отчёта хранится отдельно.


## Нативная проверка

Шесть финальных состояний на одном source:540p Tank, Spider, Factory
closed/open;1080p Factory open effects35;4K Factory open effects0.
Для540p эффекты100. Это настоящий authored map изUI644 с двумя добавленными
дружественными боевыми актёрами; исходный враг сохранён. Обе стороны используют
один и тот же RON743e821c. База646 сохранена отдельно; парные540p кадры
показывают удаление лишней полосы и уменьшение очереди. Отдельный baseline4K
доказывает, что смещение меток существовало до правки. Промежуточные926dabac
кадры с неверными метками не объявлены финальным визуальным допуском.

Финальные шесть companion JPEG просмотрены, включая отдельный4K preview;
PNG проходили `image-preview.py` перед осмотром. У Tank/Spider нет каталога
строителя. Закрытый compact drawer скрывает Row/Filters; открытый доступен,
на desktop работают четыре optional context recipes без второго Row.
Очередь читается сверху, постоянные minimap/commands на прежних местах.
Метки4K теперь привязаны к моделям. Статичные кадры paused и не являются FPS.

Существующий native motion/input runner прошёл на том же финальном source:
очередь, fan/gap, выбор, hover recipe, disclosure, Fold/Restore, Cancel,
menu/world blocking и Resume. **16** результатов ввода valid=true,
errors.txt пуст. Геометрия minimap/Stop стабильна, protected object overlaps0,
recipe overlaps0. Верхняя очередь при540p имеет ширину269 физических px
после округления268,8 UIpx; x675/top12. Высота меняется при раскрытии деталей.
Видео1× по реальным timestamps декодировано. Это функциональная приёмка,
не боевой FPS; full production exit подтверждён focused SIM/UI contracts,
не придуман по короткому native клипу.

Артефакты: `out/ui647/final-*-contact.preview.jpg`,
`visual-review-data.json`, `final-after-*/capture.json` и финальный motion
capture/video, указанный в visual-review-data. Его source совпадает с build;
старый motion clip926dabac оставлен отдельно.


## Дополнение05 и границы допуска

Повторно05 не накладывался. В646 уже восстановлен штатный физический путь
листвы `dust_materials:(0,)`, а код распространения семян не сдвигает
origin/radius. Обе `nuclear_trunk_cuts_group...` (physical и явный legacy dust)
прошли в текущем механическом646 receipt. Engine/data с тех пор не менялись.

Предложенные1588/4804мс не являются замером этого source: вариант216
доказанно ломал реальный Spider walking-blasts, поэтому оставлены64 с
сегментным broad reject. Этот отказ, причинная изоляция и реальные собственные
парные результаты не отменяются дополнительными быстрыми surface-прогонами.
См. `docs/analysis/646-physical-forest-and-snapshot-optimization-20261118.md`.

| Область | Done / remaining |
| --- | --- |
| order71 prepared/idle/AMBER | Done как уже реализованный эквивалент; повторного изменения нет |
|06 projection/Scan guards | Done: присутствовали до переноса, подтверждены реальными builders и кликами |
|06 Filters/Row и540p queue | Done: причина исправлена; red→green, idle layout и native подтверждены |
| Найденный4K DPI | Done: baseline воспроизведён, регрессия и live-scale/native проверки пройдены |
| Текущий focused/native UI | Done в указанном объёме на одном source; это не приёмка всех прежних задач |
|628/627 performance и625 scale | Remaining прежние: frontier259k, SIM~0,6×,73units не5000; ordinary/release FPS не квалифицирован |
|627 интеграционная доставка | Remaining: единый окончательный source/release/launcher после всех gates |

Новых FPS-прогонов здесь не было: промежуточный механический646 A/B не
выдан за измерение нового UI source. Для этих изменений не повторялись
дорогие полные миры/архивные эксперименты. Не заявляется no-regression
допуск всего боя или всех незакрытых задач. Ordinary FPS остаётся открытым
согласно выбору пользователя о фоновых замерах.

Новый development binary заморожен в `out/ui647/blast-ui647.exe`.
Release в этом исправлении не собирался: общий этап стабилизации/доставки627
ещё не прошёл. «Полигон» и canonical627M не переключались. Не изменялись
queue/steer, порядок очереди, context limits или архивный диалог.
Итоговые hashes, receipts, документация и завершение команд:
`out/ui647/final-evidence.json`, done/remaining — `out/ui647/ledger.json`.

