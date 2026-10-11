//! Ecommerce's Invoices::Invoice#invoice_items must read the namespaced
//! model, even when a top-level InvoiceItem also exists. Keep the models
//! in separate files to isolate association lookup from sibling ingestion.

use super::emit_and_run;

fn invoice_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\"",
            r#"  create_table "invoices", force: :cascade do |t|
    t.integer "article_id"
  end
  create_table "invoice_items", force: :cascade do |t|
    t.integer "invoice_id", null: false
    t.integer "value", null: false
    t.integer "vat_rate", null: false
  end
  create_table "legacy_invoice_items", force: :cascade do |t|
    t.integer "invoice_id", null: false
    t.integer "value", null: false
  end
  create_table "articles""#,
        )
        .write(
            "app/models/invoices/invoice.rb",
            r#"module Invoices
  class Invoice < ApplicationRecord
    has_many :invoice_items
    has_many :explicit_items, class_name: "InvoiceItem"
    has_many :legacy_items, class_name: "::InvoiceItem"
    has_one :first_item, class_name: "InvoiceItem"
    belongs_to :article, optional: true

    def total_value_with_tax
      invoice_items.sum(&:value_with_tax)
    end
  end
end
"#,
        )
        .write(
            "app/models/invoices/invoice_item.rb",
            r#"module Invoices
  class InvoiceItem < ApplicationRecord
    belongs_to :invoice

    def value_with_tax
      value + (value * vat_rate / 100)
    end
  end
end
"#,
        )
        .write(
            "app/models/invoice_item.rb",
            r#"class InvoiceItem < ApplicationRecord
  self.table_name = "legacy_invoice_items"

  def value_with_tax
    value + 1000
  end
end
"#,
        )
}

#[test]
fn namespaced_associations_read_the_resolved_models() {
    invoice_app().run_ruby(r#"
article = Article.create!(title: "Invoice", body: "An invoice's article.")
invoice = Invoices::Invoice.create!(article_id: article.id)
first = Invoices::InvoiceItem.create!(invoice_id: invoice.id, value: 200, vat_rate: 23)
Invoices::InvoiceItem.create!(invoice_id: invoice.id, value: 100, vat_rate: 8)
InvoiceItem.create!(invoice_id: invoice.id, value: 17)
other = Invoices::Invoice.create!(article_id: article.id)
Invoices::InvoiceItem.create!(invoice_id: other.id, value: 900, vat_rate: 10)

raise "wrong namespace or foreign key: #{invoice.total_value_with_tax}" unless invoice.total_value_with_tax == 354
raise "relative class_name" unless invoice.explicit_items.map(&:value).sort == [100, 200]
raise "absolute class_name" unless invoice.legacy_items.map(&:value) == [17]
raise "has_one target" unless invoice.first_item.value == 200
raise "belongs_to namespace" unless first.invoice.id == invoice.id
raise "top-level fallback" unless invoice.article.title == "Invoice"
puts "namespaced association targets passed"
"#,
        )
        .assert_passes();
}

/// An STI class is ingested as a library class, not a model with the
/// synthesized association operations. Qualifying it made this check
/// clean but emitted Relation#sum raised for its missing column argument.
#[test]
fn an_unmodeled_namespaced_target_keeps_its_diagnostic() {
    let (_emitted, errors) = invoice_app()
        .edit(
            "app/models/invoices/invoice.rb",
            "has_many :invoice_items",
            "has_many :special_items",
        )
        .edit(
            "app/models/invoices/invoice.rb",
            "invoice_items.sum",
            "special_items.sum",
        )
        .write(
            "app/models/invoices/special_item.rb",
            "class Invoices::SpecialItem < Invoices::InvoiceItem\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("value_with_tax") && e.contains("SpecialItem")),
        "an unmodeled target must not become clean without a working runtime: {errors:?}",
    );
}

/// HasOne has no through metadata or runtime traversal. Resolving its
/// class must not turn the discarded option into a clean direct-FK read.
#[test]
fn a_has_one_through_target_does_not_become_a_direct_association() {
    let (_emitted, errors) = invoice_app()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\", force: :cascade do |t|",
            "  create_table \"articles\", force: :cascade do |t|\n    t.integer \"invoice_item_id\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ApplicationRecord\n  belongs_to :invoice_item, class_name: \"Invoices::InvoiceItem\", optional: true",
        )
        .edit(
            "app/models/invoices/invoice.rb",
            "has_one :first_item, class_name: \"InvoiceItem\"",
            "has_one :first_item, class_name: \"InvoiceItem\", through: :article, source: :invoice_item",
        )
        .edit(
            "app/models/invoices/invoice.rb",
            "invoice_items.sum(&:value_with_tax)",
            "first_item.value_with_tax",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("has_one") && e.contains("Invoices::Invoice")),
        "has_one through must remain unsupported until it has a runtime traversal: {errors:?}",
    );
}
