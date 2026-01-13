from __future__ import annotations

from django.urls import path

from demoapp import views

urlpatterns = [
    path("baseline/", views.baseline_items),
    path("offload/", views.offload_items),
    path("compute/", views.compute_view),
    path("compute_offload/", views.compute_offload_view),
    path("offload_table/", views.offload_table),
    path("health/", views.health_view),
]
