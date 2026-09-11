# Planner bug report -- one file per field

Filed by hand at the URL below, which preselects the template, the labels, the
assignee and the title:

https://github.com/sovietspaceship/souls-bug-reports/issues/new?assignees=sovietspaceship&labels=er%20build%20planner%2Cbug&template=er_build_planner_bug_report.yml

Paste each file into the field its number names. The order matches the form.

| file | field |
|---|---|
| `title.md` | the issue title |
| `01-describe-the-bug.md` | Describe the bug |
| `02-to-reproduce.md` | To Reproduce |
| `03-expected-behavior.md` | Expected behavior |
| `04-build-or-workspace-url.md` | Build or workspace URL |
| `05-screenshots.md` | Screenshots |
| `06-device.md` | Device (dropdown: Desktop) |
| `07-browser.md` | Browser |
| `08-additional-context.md` | Additional context |

This was first drafted against `er_build_planner_feature_request.yml`, which was
the wrong form: the Cosmetics tab has had Cheat Engine AOB import and export
since v2.19 (2024-03-01), so there is no feature to request. What is broken is
the merge on load, and that is a bug.
