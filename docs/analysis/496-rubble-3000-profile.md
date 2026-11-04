# 496 — Детальный профиль rubble на 3000 тиков

3 ноября 2026. Артефакты: `out/optimization-496/`.

## Методика

World AI-vs-AI, fat-LTO ADR-836, physics8/navigation8/Rayon4, `--profile-rubble`.
Parent spans inclusive и пересекаются, поэтому их нельзя складывать друг с другом
или напрямую с `rubble_us`. Это диагностика состава работы, не новый A/B результат.

## Позднее окно 2701–3000

| Стадия | Inclusive ns/тик |
|---|---:|
|`physics_rubble_support_build`|6 022 210|
|`physics_rubble_classify`|1 990 920|
|`physics_rubble_support_query`|412 809|
|`physics_rubble_columns`|534 355|
|`physics_rubble_gather_dispatch`|426 199|
|`physics_rubble_merge`|106 723|

Полный тик окна49,515 мс, `debris`27,066 мс. Главная стоимость rubble — второй
полный `Settled` builder, а не чтение support queries. Запрос-driven двухпроходный
builder (465), support proofs (452), persistent indexes и sorted cells (495) уже
отклонены и не повторяются.

Публикация rubble не меняется в208 из300 поздних тиков, но это не доказывает,
что входы builder неизменны: flying/landing, опоры и hull poses продолжают меняться.
Простой пропуск по равенству прошлой публикации был бы неточным.

## Следующий срез

В основном и rubble `Settled` каждая связь сейчас создаётся на каждую Y-ячейку.
Проверяется компактная вертикальная маска: нода на XZ и 64-ячеечное слово высоты,
с точным `u64` набором запрошенных Y. Это уменьшает работу обоих builders без
persistent state, sorting, superset или изменения порядка контактов.
